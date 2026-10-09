//! Durable conflict decisions and private comparison copies. The sync owner
//! executes decisions between cycles; settings clients only enqueue instructions.
use crate::{db::Database, executor, local, profile, remote, *};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

fn stale() -> Error {
    Error::new(
        ErrorCode::Conflict,
        "The file versions changed. Refresh the comparison before choosing a version; both files are preserved.",
    )
}
fn plain_file(value: Option<&LocalEntry>) -> Result<&LocalEntry> {
    value.filter(|v| v.kind == ItemKind::File && v.fingerprint.is_some()).ok_or_else(|| Error::new(ErrorCode::Unsupported, "This action requires two ordinary files. Refresh missing, moved or ambiguous items first."))
}
async fn root_healthy(pair: &PairConfig, provider: &dyn Provider) -> Result<()> {
    let root = local::canonical_root(&pair.local_root)?;
    if local::identity(&fs::metadata(root)?).as_deref() != Some(&pair.root_identity) {
        return Err(stale());
    }
    let root = provider.get(&pair.remote_root_id).await?;
    if root.kind != ItemKind::Folder || root.trashed {
        return Err(stale());
    }
    Ok(())
}
async fn versions(
    pair: &PairConfig,
    provider: &dyn Provider,
    conflict: &Conflict,
) -> Result<(LocalEntry, RemoteItem)> {
    root_healthy(pair, provider).await?;
    let expected = plain_file(conflict.local.as_ref())?;
    let actual = local::entry(&pair.local_root, &conflict.path)?.ok_or_else(stale)?;
    if !actual.content_eq(expected) || actual.file_identity != expected.file_identity {
        return Err(stale());
    }
    let expected = conflict
        .remote
        .as_ref()
        .filter(|v| v.kind == ItemKind::File && v.fingerprint.is_some())
        .ok_or_else(stale)?;
    let remote = provider.get(&expected.id).await?;
    // Metadata-only Drive version bumps do not change the displayed bytes. The
    // queued write still uses this freshly fetched version/ETag conditionally.
    if remote.trashed
        || !remote.content_eq(expected)
        || remote.parents != expected.parents
        || remote.name != expected.name
        || remote::ancestry(provider, &remote.id, &pair.remote_root_id).await?
            != remote::Ancestry::Inside
    {
        return Err(stale());
    }
    Ok((actual, remote))
}
fn private_output(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    Ok(options.open(path)?)
}
fn managed_directory(profile_root: &Path, category: &str, id: &str) -> Result<std::path::PathBuf> {
    uuid::Uuid::parse_str(id).map_err(|_| stale())?;
    let parent = profile_root.join(category);
    let directory = parent.join(id);
    for path in [&parent, &directory] {
        if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(stale());
        }
        profile::private_directory(path)?;
    }
    if !directory
        .canonicalize()?
        .starts_with(profile_root.canonicalize()?)
    {
        return Err(stale());
    }
    Ok(directory)
}
async fn remote_copy(
    db: &Database,
    pair: &PairConfig,
    provider: &dyn Provider,
    conflict: &Conflict,
    item: &RemoteItem,
    path: &Path,
) -> Result<()> {
    let fingerprint = item.fingerprint.as_ref().ok_or_else(stale)?;
    if path.exists() {
        if fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(stale());
        }
        if local::fingerprint(path)? == *fingerprint {
            return Ok(());
        }
        // An interrupted partial copy belongs only to this job. Never truncate
        // a completed backup or an unrelated file.
        if fs::symlink_metadata(path)?.file_type().is_symlink()
            || fs::metadata(path)?.len() >= fingerprint.size
        {
            return Err(stale());
        }
        fs::remove_file(path)?;
    }
    let mut output = private_output(path)?;
    let mut offset = 0;
    while offset < fingerprint.size {
        if db.controls()?.quit {
            return Err(Error::new(
                ErrorCode::Cancelled,
                "Comparison preparation will resume when FreeSync starts again.",
            ));
        }
        let bytes = provider
            .download(
                &item.id,
                offset,
                (fingerprint.size - offset).min(1024 * 1024) as usize,
            )
            .await?;
        if bytes.is_empty() {
            return Err(Error::new(
                ErrorCode::Transient,
                "The comparison download was incomplete. Try the action again.",
            ));
        }
        output.write_all(&bytes)?;
        offset += bytes.len() as u64;
    }
    output.sync_all()?;
    drop(output);
    if local::fingerprint(path)? != *fingerprint {
        return Err(stale());
    }
    versions(pair, provider, conflict).await?;
    Ok(())
}
fn local_copy(
    pair: &PairConfig,
    conflict: &Conflict,
    original: &LocalEntry,
    path: &Path,
) -> Result<()> {
    if !path.exists() {
        let source = local::safe_join(&pair.local_root, &conflict.path)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut input = options.open(source)?;
        let mut output = private_output(path)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
    }
    let actual = local::entry(&pair.local_root, &conflict.path)?.ok_or_else(stale)?;
    if !actual.content_eq(original)
        || actual.file_identity != original.file_identity
        || local::fingerprint(path)? != *original.fingerprint.as_ref().ok_or_else(stale)?
    {
        return Err(stale());
    }
    Ok(())
}
fn allowed(character: char) -> bool {
    !character.is_control() || matches!(character, '\t' | '\n' | '\r' | '\u{000c}')
}
/// Validate complete UTF-8 or BOM-marked UTF-16 content without loading a large
/// file into memory. NULs, malformed encoding and binary controls are rejected.
pub fn is_text_file(path: &Path) -> Result<bool> {
    let mut input = File::open(path)?;
    let mut bom = [0; 2];
    let count = input.read(&mut bom)?;
    if count == 2 && matches!(bom, [0xff, 0xfe] | [0xfe, 0xff]) {
        if input.metadata()?.len() % 2 != 0 {
            return Ok(false);
        }
        let little = bom == [0xff, 0xfe];
        let mut reader = std::io::BufReader::new(input);
        let mut read_error = false;
        let units = std::iter::from_fn(|| {
            let mut bytes = [0; 2];
            match reader.read_exact(&mut bytes) {
                Ok(()) => Some(if little {
                    u16::from_le_bytes(bytes)
                } else {
                    u16::from_be_bytes(bytes)
                }),
                Err(error) => {
                    if error.kind() != std::io::ErrorKind::UnexpectedEof {
                        read_error = true;
                    }
                    None
                }
            }
        });
        let valid = char::decode_utf16(units).all(|c| c.is_ok_and(allowed));
        return Ok(valid && !read_error);
    }
    input.seek(SeekFrom::Start(0))?;
    let mut buffer = vec![0; 64 * 1024 + 4];
    let mut carried = 0;
    loop {
        let count = input.read(&mut buffer[carried..64 * 1024])?;
        if count == 0 {
            return Ok(carried == 0);
        }
        let length = carried + count;
        match std::str::from_utf8(&buffer[..length]) {
            Ok(text) => {
                if !text.chars().all(allowed) {
                    return Ok(false);
                }
                carried = 0;
            }
            Err(error) => {
                if error.error_len().is_some() {
                    return Ok(false);
                }
                let valid = error.valid_up_to();
                if !std::str::from_utf8(&buffer[..valid])
                    .map_err(|_| stale())?
                    .chars()
                    .all(allowed)
                {
                    return Ok(false);
                }
                carried = length - valid;
                buffer.copy_within(valid..length, 0);
            }
        }
    }
}
async fn refresh(
    db: &mut Database,
    pair: &PairConfig,
    provider: &dyn Provider,
    conflict: &Conflict,
) -> Result<()> {
    root_healthy(pair, provider).await?;
    let local = local::scan(&pair.local_root, &pair.excludes)?;
    let remote = remote::snapshot(
        provider,
        &pair.remote_root_id,
        &pair.excludes,
        &db.baselines(&pair.id)?,
    )
    .await?;
    let l = local.entries.get(&conflict.path).cloned();
    let r = remote
        .entries
        .get(&conflict.path)
        .filter(|items| items.len() == 1)
        .and_then(|items| items.first())
        .cloned();
    if let (Some(l), Some(r)) = (&l, &r)
        && r.matches_local(l)
    {
        db.replace_conflict(
            conflict,
            None,
            Some(&Baseline {
                path: conflict.path.clone(),
                local: l.clone(),
                remote: r.clone(),
            }),
        )?;
    } else {
        let fresh=Conflict{id:uuid::Uuid::new_v4().to_string(),pair_id:pair.id.clone(),path:conflict.path.clone(),reason:"Fresh comparison: the versions still differ or an item is missing/ambiguous. Choose a version or leave this unresolved.".into(),local:l,remote:r};
        db.replace_conflict(conflict, Some(&fresh), None)?;
    }
    Ok(())
}
async fn prepare(
    db: &mut Database,
    profile_root: &Path,
    pair: &PairConfig,
    provider: &dyn Provider,
    task: &mut ConflictTask,
) -> Result<()> {
    if let Some(operation) = db.operation(&task.id)? {
        task.operation_id = Some(operation.id);
        task.state = "syncing".into();
        return Ok(());
    }
    if task.choice == ConflictChoice::KeepBoth
        && db
            .get::<bool>(&format!("resolution_done:{}", task.conflict.id))?
            .unwrap_or(false)
    {
        task.preserved_path = db
            .get::<(String, LocalEntry)>(&format!("resolution:{}", task.conflict.id))?
            .map(|v| v.0);
        task.state = "syncing".into();
        return Ok(());
    }
    let current = db
        .conflicts(&pair.id)?
        .into_iter()
        .find(|c| c.id == task.conflict.id)
        .ok_or_else(stale)?;
    if current.path != task.conflict.path
        || serde_json::to_value(&current.local)? != serde_json::to_value(&task.conflict.local)?
        || serde_json::to_value(&current.remote)? != serde_json::to_value(&task.conflict.remote)?
    {
        return Err(stale());
    }
    if task.choice == ConflictChoice::Refresh {
        refresh(db, pair, provider, &current).await?;
        task.state = "done".into();
        return Ok(());
    }
    if db.operations(&pair.id)?.iter().any(|o| {
        crate::planner::beneath(&o.path, &current.path)
            || crate::planner::beneath(&current.path, &o.path)
    }) {
        return Err(Error::new(
            ErrorCode::Conflict,
            "A transfer still targets this item. Try again after it finishes.",
        ));
    }
    let (local, remote) = versions(pair, provider, &current).await?;
    match task.choice {
        ConflictChoice::KeepBoth => {
            task.preserved_path =
                Some(executor::keep_both_owned(db, pair, provider, &current.path).await?);
            task.state = "syncing".into();
        }
        ConflictChoice::UseLocal | ConflictChoice::UseDrive => {
            if task.choice == ConflictChoice::UseLocal {
                if !remote.can_edit || !remote.can_download {
                    return Err(Error::new(
                        ErrorCode::Permission,
                        "The Drive version must be editable and downloadable so its original can be retained.",
                    ));
                }
                let directory = managed_directory(profile_root, "recovery", &task.id)?;
                remote_copy(
                    db,
                    pair,
                    provider,
                    &current,
                    &remote,
                    &directory.join("drive-original"),
                )
                .await?;
                let metadata = directory.join("manifest.json");
                if !metadata.exists() {
                    let mut output = private_output(&metadata)?;
                    output.write_all(&serde_json::to_vec(&serde_json::json!({"pair":pair.id,"path":current.path,"saved_at":executor::now(),"version":"Drive original","content":"drive-original"}))?)?;
                    output.sync_all()?;
                }
            } else if !remote.can_download {
                return Err(Error::new(
                    ErrorCode::Permission,
                    "The Drive version cannot be downloaded.",
                ));
            }
            let operation = Operation {
                id: task.id.clone(),
                pair_id: pair.id.clone(),
                path: current.path.clone(),
                action: if task.choice == ConflictChoice::UseLocal {
                    Action::Upload
                } else {
                    Action::Download
                },
                expected_local: Some(local),
                expected_remote: Some(remote),
                state: OperationState::Prepared,
                attempts: 0,
                retry_at: 0,
                upload_session: None,
                reserved_remote_id: None,
                error: None,
            };
            db.queue_conflict_operation(&current, &operation)?;
            task.operation_id = Some(operation.id);
            task.state = "syncing".into();
        }
        ConflictChoice::Compare => {
            if !remote.can_download {
                return Err(Error::new(
                    ErrorCode::Permission,
                    "The Drive version cannot be downloaded.",
                ));
            }
            let directory = managed_directory(profile_root, "comparisons", &task.id)?;
            task.comparison_directory = Some(directory.clone());
            local_copy(pair, &current, &local, &directory.join("local.txt"))?;
            remote_copy(
                db,
                pair,
                provider,
                &current,
                &remote,
                &directory.join("drive.txt"),
            )
            .await?;
            if !is_text_file(&directory.join("local.txt"))?
                || !is_text_file(&directory.join("drive.txt"))?
            {
                return Err(Error::new(
                    ErrorCode::Unsupported,
                    "Diff is available for text files (UTF-8 or UTF-16), not binary content.",
                ));
            }
            for file in ["local.txt", "drive.txt"] {
                let path = directory.join(file);
                let mut permissions = fs::metadata(&path)?.permissions();
                permissions.set_readonly(true);
                fs::set_permissions(path, permissions)?;
            }
            task.state = "ready_to_open".into();
        }
        ConflictChoice::Refresh => unreachable!(),
    }
    Ok(())
}
/// Called only by the profile owner. Settings remain responsive while this runs.
pub async fn process_tasks(
    db: &mut Database,
    profile_root: &Path,
    factory: &dyn crate::engine::ProviderFactory,
) -> Result<()> {
    for mut task in db.conflict_tasks()? {
        if db.controls()?.quit {
            break;
        }
        if !matches!(task.state.as_str(), "queued" | "working" | "syncing") {
            continue;
        }
        let pair = db
            .pairs()?
            .into_iter()
            .find(|p| p.id == task.conflict.pair_id)
            .ok_or_else(stale)?;
        if task.state == "syncing" {
            if let Some(id) = &task.operation_id {
                if let Some(operation) = db.operation(id)? {
                    match operation.state {
                        OperationState::Done => task.state = "done".into(),
                        OperationState::Conflict => {
                            task.state = "failed".into();
                            task.error = operation.error;
                        }
                        _ => continue,
                    }
                }
            } else if let Some(preserved) = &task.preserved_path {
                let baselines = db.baselines(&pair.id)?;
                if db
                    .conflicts(&pair.id)?
                    .iter()
                    .any(|c| c.path == task.conflict.path || &c.path == preserved)
                {
                    task.state = "failed".into();
                    task.error = Some(Error::new(
                        ErrorCode::Conflict,
                        "A preserved version changed again. Review the new conflict; both versions remain preserved.",
                    ));
                } else if baselines.iter().any(|b| b.path == task.conflict.path)
                    && baselines.iter().any(|b| &b.path == preserved)
                    && !db
                        .operations(&pair.id)?
                        .iter()
                        .any(|o| o.path == task.conflict.path || &o.path == preserved)
                {
                    task.state = "done".into();
                } else {
                    continue;
                }
            }
            db.save_conflict_task(&task)?;
            continue;
        }
        task.state = "working".into();
        db.save_conflict_task(&task)?;
        let result = async {
            let provider = factory.connect(&pair).await?;
            prepare(db, profile_root, &pair, provider.as_ref(), &mut task).await
        }
        .await;
        if let Err(error) = result {
            if error.code != ErrorCode::Cancelled
                && task.choice == ConflictChoice::Compare
                && let Some(directory) = &task.comparison_directory
            {
                // Only copies made inside this UUID job directory are removed.
                let _ = fs::remove_dir_all(directory);
            }
            task.state = if error.code == ErrorCode::Cancelled {
                "queued"
            } else {
                "failed"
            }
            .into();
            task.error = Some(error);
        }
        db.save_conflict_task(&task)?;
    }
    Ok(())
}
