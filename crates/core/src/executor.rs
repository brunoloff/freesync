//! A bounded (one-operation) durable executor. Staged content and displaced files
//! survive interruption; no local replacement uses an overwriting rename.
use crate::{
    db::Database,
    local,
    planner::beneath,
    provider::{UploadProgress, UploadRequest},
    remote::{self, Ancestry},
    *,
};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio_util::sync::CancellationToken;

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn changed() -> Error {
    Error::new(
        ErrorCode::Conflict,
        "Content or location changed during this operation. Both versions are preserved.",
    )
}
fn expected<T>(value: &Option<T>) -> Result<&T> {
    value.as_ref().ok_or_else(|| {
        Error::new(
            ErrorCode::Internal,
            "The queued operation is missing its original state.",
        )
    })
}
fn local_matches(actual: Option<&LocalEntry>, expected: Option<&LocalEntry>) -> bool {
    match (actual, expected) {
        (None, None) => true,
        (Some(a), Some(b)) => a.content_eq(b) && a.file_identity == b.file_identity,
        _ => false,
    }
}
fn check_local(
    pair: &PairConfig,
    path: &str,
    value: Option<&LocalEntry>,
) -> Result<Option<LocalEntry>> {
    let actual = local::entry(&pair.local_root, path)?;
    if !local_matches(actual.as_ref(), value) {
        return Err(changed());
    }
    Ok(actual)
}
fn checkpoint(db: &Database, cancel: &CancellationToken) -> Result<()> {
    let controls = db.controls()?;
    if cancel.is_cancelled() || controls.paused || controls.quit {
        return Err(Error::new(
            ErrorCode::Cancelled,
            "Sync is paused or stopping. Progress is saved.",
        ));
    }
    Ok(())
}
fn progress(db: &Database, pair: &PairConfig, op: &Operation, done: u64, total: u64) -> Result<()> {
    db.transfer_progress(op, done, total)?;
    let mut status = crate::engine::status(db, pair, "syncing", None)?;
    status.current_path = Some(op.path.clone());
    status.progress_bytes = done;
    status.total_bytes = total;
    status.retry_at = None;
    status.error = None;
    db.set_status(&status)
}
async fn healthy(pair: &PairConfig, provider: &dyn Provider) -> Result<()> {
    let root = local::canonical_root(&pair.local_root)?;
    if local::identity(&fs::metadata(root)?).is_some_and(|id| id != pair.root_identity) {
        return Err(Error::new(
            ErrorCode::IncompleteScan,
            "The local root was replaced. Sync is stopped.",
        ));
    }
    let root = provider.get(&pair.remote_root_id).await?;
    if root.trashed || root.kind != ItemKind::Folder {
        return Err(Error::new(
            ErrorCode::IncompleteScan,
            "The Drive root is unavailable. Sync is stopped.",
        ));
    }
    Ok(())
}
async fn check_remote(
    pair: &PairConfig,
    provider: &dyn Provider,
    value: &RemoteItem,
) -> Result<RemoteItem> {
    let actual = provider.get(&value.id).await?;
    if actual.trashed
        || actual.version != value.version
        || actual.parents != value.parents
        || actual.name != value.name
        || !actual.content_eq(value)
        || remote::ancestry(provider, &actual.id, &pair.remote_root_id).await? != Ancestry::Inside
    {
        return Err(changed());
    }
    Ok(actual)
}
/// A downloaded source can receive a metadata-only version bump after Drive
/// finishes an upload. Comparable bytes, identity and location must still agree.
/// Write destinations continue using strict version/ETag checks above.
async fn check_download_source(
    pair: &PairConfig,
    provider: &dyn Provider,
    value: &RemoteItem,
) -> Result<RemoteItem> {
    let actual = provider.get(&value.id).await?;
    if actual.trashed
        || actual.parents != value.parents
        || actual.name != value.name
        || actual.kind != value.kind
        || actual.fingerprint.is_none()
        || !actual.content_eq(value)
        || remote::ancestry(provider, &actual.id, &pair.remote_root_id).await? != Ancestry::Inside
    {
        return Err(changed());
    }
    Ok(actual)
}
async fn parent(
    db: &Database,
    pair: &PairConfig,
    provider: &dyn Provider,
    path: &str,
) -> Result<(String, String)> {
    let (prefix, name) = path.rsplit_once('/').unwrap_or(("", path));
    let id = if prefix.is_empty() {
        pair.remote_root_id.clone()
    } else {
        db.baselines(&pair.id)?
            .into_iter()
            .find(|b| b.path == prefix && b.remote.kind == ItemKind::Folder)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::Conflict,
                    "The parent folder is not reconciled. Resolve its conflict first.",
                )
            })?
            .remote
            .id
    };
    let actual = provider.get(&id).await?;
    if actual.trashed
        || actual.kind != ItemKind::Folder
        || !actual.can_add_children
        || remote::ancestry(provider, &id, &pair.remote_root_id).await? != Ancestry::Inside
    {
        return Err(changed());
    }
    Ok((id, name.into()))
}
fn private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true).read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    Ok(options.open(path)?)
}
fn stage_path(profile: &Path, operation: &Operation, suffix: &str) -> Result<PathBuf> {
    let dir = profile.join("staging");
    crate::profile::private_directory(&dir)?;
    Ok(dir.join(format!("{}.{}", operation.id, suffix)))
}
/// Move the original inode out of sync, so an editor holding it open continues
/// writing to recovery rather than to an unlinked or overwritten file.
fn recycle(
    db: &Database,
    profile: &Path,
    pair: &PairConfig,
    op: &Operation,
    path: &str,
) -> Result<()> {
    let dir = profile.join("recovery").join(&op.id);
    crate::profile::private_directory(&dir)?;
    let backup = dir.join("content");
    let actual = local::entry(&pair.local_root, path)?;
    if backup.exists() {
        if actual.is_none() {
            return Ok(());
        }
        return Err(changed());
    }
    check_local(pair, path, op.expected_local.as_ref())?;
    let source = local::safe_join(&pair.local_root, path)?;
    if actual.as_ref().is_some_and(|l| l.kind == ItemKind::Folder)
        && fs::read_dir(&source)?.next().is_some()
    {
        return Err(changed());
    }
    db.set(&format!("recovery:{}",op.id),&serde_json::json!({"pair":pair.id,"path":path,"saved_at":now(),"location":backup,"original":op.expected_local}))?;
    fs::rename(source, &backup).map_err(|e| {
        if e.raw_os_error() == Some(18) {
            Error::new(
                ErrorCode::Unsupported,
                "Recovery and sync folders must be on the same filesystem in this preview.",
            )
        } else {
            e.into()
        }
    })?;
    if op
        .expected_local
        .as_ref()
        .is_some_and(|l| l.kind == ItemKind::File)
        && local::fingerprint(&backup)?
            != expected(&op.expected_local)?
                .fingerprint
                .clone()
                .ok_or_else(changed)?
    {
        return Err(changed());
    }
    Ok(())
}
fn install(stage: &Path, destination: &Path) -> Result<()> {
    // hard_link is an atomic create-if-absent. An intervening editor save wins.
    fs::hard_link(stage, destination).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => changed(),
        _ if e.raw_os_error() == Some(18) => Error::new(
            ErrorCode::Unsupported,
            "Staging and sync folders must be on the same filesystem in this preview.",
        ),
        _ => e.into(),
    })?;
    #[cfg(unix)]
    File::open(destination.parent().ok_or_else(changed)?)?.sync_all()?;
    Ok(())
}
fn move_no_replace(source: &Path, destination: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let source =
            std::ffi::CString::new(source.as_os_str().as_bytes()).map_err(|_| changed())?;
        let destination =
            std::ffi::CString::new(destination.as_os_str().as_bytes()).map_err(|_| changed())?;
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source.as_ptr(),
                libc::AT_FDCWD,
                destination.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(target_os = "macos")]
        let result =
            unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) };
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let result = -1;
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        Err(if error.kind() == std::io::ErrorKind::AlreadyExists {
            changed()
        } else {
            error.into()
        })
    }
    #[cfg(not(unix))]
    {
        fs::rename(source, destination)?;
        Ok(())
    }
}
fn baseline(pair: &PairConfig, path: &str, remote: RemoteItem) -> Result<Baseline> {
    let local = local::entry(&pair.local_root, path)?.ok_or_else(changed)?;
    if !remote.matches_local(&local) {
        return Err(changed());
    }
    Ok(Baseline {
        path: path.into(),
        local,
        remote,
    })
}
fn finish_move(
    db: &mut Database,
    pair: &PairConfig,
    op: &Operation,
    from: &str,
    item: RemoteItem,
) -> Result<()> {
    let mut moved = vec![];
    for mut b in db
        .baselines(&pair.id)?
        .into_iter()
        .filter(|b| beneath(&b.path, from))
    {
        let old = b.path.clone();
        b.path = format!("{}{}", op.path, &old[from.len()..]);
        b.local.path = b.path.clone();
        if old == from {
            b.remote = item.clone();
        }
        moved.push(b);
    }
    if moved.is_empty() {
        moved.push(baseline(pair, &op.path, item)?);
    }
    db.finish_many(op, &moved, Some(from))
}

async fn upload(
    db: &mut Database,
    profile: &Path,
    pair: &PairConfig,
    provider: &dyn Provider,
    op: &mut Operation,
    cancel: &CancellationToken,
    chunk_size: usize,
) -> Result<RemoteItem> {
    let source = expected(&op.expected_local)?.clone();
    let fingerprint = source.fingerprint.as_ref().ok_or_else(changed)?;
    let (parent, name) = parent(db, pair, provider, &op.path).await?;
    // A response may have been lost after final commit. Match the journaled ID
    // and private operation marker before starting another session.
    let target = op
        .expected_remote
        .as_ref()
        .map(|r| r.id.clone())
        .or_else(|| op.reserved_remote_id.clone());
    if let Some(id) = target {
        match provider.get(&id).await {
            Ok(item)
                if item.operation_id.as_deref() == Some(&op.id)
                    && item.fingerprint.as_ref() == Some(fingerprint)
                    && item.parents.contains(&parent)
                    && item.name == name
                    && !item.trashed =>
            {
                check_local(pair, &op.path, Some(&source))?;
                return Ok(item);
            }
            Ok(_)
            | Err(Error {
                code: ErrorCode::NotFound,
                ..
            }) => (),
            Err(e) => return Err(e),
        }
    }
    check_local(pair, &op.path, Some(&source))?;
    let staging = stage_path(profile, op, "source")?;
    if staging.exists() && local::fingerprint(&staging)? != *fingerprint {
        fs::remove_file(&staging)?;
    }
    if !staging.exists() {
        let mut output = private_file(&staging)?;
        let mut input = File::open(local::safe_join(&pair.local_root, &op.path)?)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
    }
    if local::fingerprint(&staging)? != *fingerprint {
        return Err(changed());
    }
    check_local(pair, &op.path, Some(&source))?;
    if op.expected_remote.is_none() && op.reserved_remote_id.is_none() {
        op.reserved_remote_id = Some(provider.reserve_id().await?);
        db.save_operation(op)?;
    }
    if let Some(session) = &op.upload_session {
        match provider.upload_status(session).await {
            Ok(UploadProgress::Complete(item)) => return Ok(item),
            Ok(UploadProgress::Continue(n)) => {
                op.upload_session.as_mut().unwrap().uploaded_bytes = n
            }
            Err(e) if e.code == ErrorCode::NotFound => op.upload_session = None,
            Err(e) => return Err(e),
        }
    }
    if op.upload_session.is_none() {
        if let Some(remote) = &op.expected_remote {
            check_remote(pair, provider, remote).await?;
        }
        op.upload_session = Some(
            provider
                .start_upload(&UploadRequest {
                    operation_id: op.id.clone(),
                    parent_id: parent,
                    name,
                    existing: op.expected_remote.clone(),
                    size: fingerprint.size,
                    reserved_id: op.reserved_remote_id.clone(),
                })
                .await?,
        );
        db.save_operation(op)?;
    }
    loop {
        checkpoint(db, cancel)?;
        check_local(pair, &op.path, Some(&source))?;
        let session = op.upload_session.as_ref().unwrap().clone();
        if session.uploaded_bytes > fingerprint.size {
            return Err(changed());
        }
        let amount = (fingerprint.size - session.uploaded_bytes).min(chunk_size as u64) as usize;
        let mut input = File::open(&staging)?;
        input.seek(SeekFrom::Start(session.uploaded_bytes))?;
        let mut bytes = vec![0; amount];
        input.read_exact(&mut bytes)?;
        match provider.upload_chunk(&session, bytes).await? {
            UploadProgress::Continue(n) => {
                if n <= session.uploaded_bytes || n > fingerprint.size {
                    return Err(changed());
                }
                op.upload_session.as_mut().unwrap().uploaded_bytes = n;
                db.save_operation(op)?;
                progress(db, pair, op, n, fingerprint.size)?;
            }
            UploadProgress::Complete(item) => {
                check_local(pair, &op.path, Some(&source))?;
                if item.fingerprint.as_ref() != Some(fingerprint) {
                    return Err(changed());
                }
                return Ok(item);
            }
        }
    }
}
async fn perform(
    db: &mut Database,
    profile: &Path,
    pair: &PairConfig,
    provider: &dyn Provider,
    op: &mut Operation,
    cancel: &CancellationToken,
    chunk_size: usize,
) -> Result<()> {
    healthy(pair, provider).await?;
    checkpoint(db, cancel)?;
    match &op.action.clone() {
        Action::Upload => {
            let item = upload(db, profile, pair, provider, op, cancel, chunk_size).await?;
            db.finish_operation(op, Some(&baseline(pair, &op.path, item)?), None)?;
        }
        Action::CreateRemoteFolder => {
            check_local(pair, &op.path, op.expected_local.as_ref())?;
            let (parent, name) = parent(db, pair, provider, &op.path).await?;
            if op.reserved_remote_id.is_none() {
                op.reserved_remote_id = Some(provider.reserve_id().await?);
                db.save_operation(op)?;
            }
            let id = op.reserved_remote_id.as_ref().unwrap();
            let item = match provider.get(id).await {
                Ok(i)
                    if i.operation_id.as_deref() == Some(&op.id)
                        && !i.trashed
                        && i.kind == ItemKind::Folder
                        && i.parents.contains(&parent)
                        && i.name == name =>
                {
                    i
                }
                Ok(_) => return Err(changed()),
                Err(e) if e.code == ErrorCode::NotFound => {
                    provider.create_folder(&parent, &name, &op.id, id).await?
                }
                Err(e) => return Err(e),
            };
            db.finish_operation(op, Some(&baseline(pair, &op.path, item)?), None)?;
        }
        Action::CreateLocalFolder => {
            let item = check_remote(pair, provider, expected(&op.expected_remote)?).await?;
            if local::entry(&pair.local_root, &op.path)?.is_none() {
                fs::create_dir(local::safe_join(&pair.local_root, &op.path)?)?;
            }
            db.finish_operation(op, Some(&baseline(pair, &op.path, item)?), None)?;
        }
        Action::Download => {
            let item =
                check_download_source(pair, provider, expected(&op.expected_remote)?).await?;
            let fingerprint = item.fingerprint.as_ref().ok_or_else(changed)?;
            if local::entry(&pair.local_root, &op.path)?
                .as_ref()
                .is_some_and(|l| item.matches_local(l))
            {
                db.finish_operation(op, Some(&baseline(pair, &op.path, item)?), None)?;
                return Ok(());
            }
            let stage = stage_path(profile, op, "download")?;
            if !stage.exists() {
                private_file(&stage)?;
            }
            let mut output = OpenOptions::new().append(true).open(&stage)?;
            let mut offset = output.metadata()?.len();
            if offset > fingerprint.size {
                return Err(changed());
            }
            while offset < fingerprint.size {
                checkpoint(db, cancel)?;
                let bytes = provider
                    .download(
                        &item.id,
                        offset,
                        (fingerprint.size - offset).min(chunk_size as u64) as usize,
                    )
                    .await?;
                if bytes.is_empty() {
                    return Err(Error::new(
                        ErrorCode::Transient,
                        "The download was incomplete. Its stage is saved for retry.",
                    ));
                }
                output.write_all(&bytes)?;
                output.sync_all()?;
                offset += bytes.len() as u64;
                progress(db, pair, op, offset, fingerprint.size)?;
            }
            output.sync_all()?;
            drop(output);
            if local::fingerprint(&stage)? != *fingerprint {
                return Err(changed());
            }
            let item = check_download_source(pair, provider, &item).await?;
            checkpoint(db, cancel)?;
            let recovery = profile.join("recovery").join(&op.id).join("content");
            if op.expected_local.is_some() {
                recycle(db, profile, pair, op, &op.path)?;
            } else if !recovery.exists() {
                check_local(pair, &op.path, None)?;
            }
            install(&stage, &local::safe_join(&pair.local_root, &op.path)?)?;
            db.finish_operation(op, Some(&baseline(pair, &op.path, item)?), None)?;
        }
        Action::MoveRemote { from } => {
            check_local(pair, &op.path, op.expected_local.as_ref())?;
            let (parent, name) = parent(db, pair, provider, &op.path).await?;
            let original = expected(&op.expected_remote)?;
            let current = provider.get(&original.id).await?;
            let item = if current.name == name
                && current.parents.contains(&parent)
                && current.content_eq(original)
                && !current.trashed
            {
                current
            } else {
                check_remote(pair, provider, original).await?;
                for b in db
                    .baselines(&pair.id)?
                    .iter()
                    .filter(|b| beneath(&b.path, from))
                {
                    let path = format!("{}{}", op.path, &b.path[from.len()..]);
                    check_local(pair, &path, Some(&b.local))?;
                    check_remote(pair, provider, &b.remote).await?;
                }
                provider.move_item(original, &parent, &name).await?
            };
            finish_move(db, pair, op, from, item)?;
        }
        Action::MoveLocal { from } => {
            let item = check_remote(pair, provider, expected(&op.expected_remote)?).await?;
            if local::entry(&pair.local_root, from)?.is_some() {
                let destination_state = local::entry(&pair.local_root, &op.path)?;
                if item.kind == ItemKind::File
                    && destination_state
                        .as_ref()
                        .is_some_and(|d| local_matches(Some(d), op.expected_local.as_ref()))
                {
                    check_local(pair, from, op.expected_local.as_ref())?;
                    fs::remove_file(local::safe_join(&pair.local_root, from)?)?;
                    finish_move(db, pair, op, from, item)?;
                    return Ok(());
                }
                check_local(pair, &op.path, None)?;
                for b in db
                    .baselines(&pair.id)?
                    .iter()
                    .filter(|b| beneath(&b.path, from))
                {
                    check_local(pair, &b.path, Some(&b.local))?;
                }
                // Destination is revalidated immediately; file moves use no-overwrite links.
                let source = local::safe_join(&pair.local_root, from)?;
                let destination = local::safe_join(&pair.local_root, &op.path)?;
                if item.kind == ItemKind::File {
                    install(&source, &destination)?;
                    fs::remove_file(source)?;
                } else {
                    move_no_replace(&source, &destination)?;
                }
            }
            if !item.matches_local(&local::entry(&pair.local_root, &op.path)?.ok_or_else(changed)?)
            {
                return Err(changed());
            }
            finish_move(db, pair, op, from, item)?;
        }
        Action::TrashRemote => {
            check_local(pair, &op.path, None)?;
            let original = expected(&op.expected_remote)?;
            let actual = provider.get(&original.id).await?;
            if !actual.trashed {
                check_remote(pair, provider, original).await?;
                if actual.kind == ItemKind::Folder
                    && !provider.children(&actual.id, None).await?.items.is_empty()
                {
                    return Err(changed());
                }
                provider.trash(original).await?;
            }
            db.finish_operation(op, None, Some(&op.path))?;
        }
        Action::RecycleLocal => {
            let actual = provider.get(&expected(&op.expected_remote)?.id).await?;
            if !actual.trashed
                && remote::ancestry(provider, &actual.id, &pair.remote_root_id).await?
                    != Ancestry::Outside
            {
                return Err(changed());
            }
            recycle(db, profile, pair, op, &op.path)?;
            db.finish_operation(op, None, Some(&op.path))?;
        }
    }
    for suffix in ["source", "download"] {
        let path = stage_path(profile, op, suffix)?;
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

/// Preserve the current local file under a stable conflict name, then let normal
/// reconciliation download the Drive version and upload the preserved local copy.
/// The owner pauses transfers while this small local transaction is performed.
pub async fn keep_both(
    db: &mut Database,
    pair: &PairConfig,
    provider: &dyn Provider,
    path: &str,
) -> Result<String> {
    if pair.enabled && !db.controls()?.paused {
        return Err(Error::new(
            ErrorCode::InvalidConfig,
            "Pause sync before resolving this conflict.",
        ));
    }
    keep_both_owned(db, pair, provider, path).await
}

/// The engine calls this only between cycles while holding the profile lock.
pub(crate) async fn keep_both_owned(
    db: &mut Database,
    pair: &PairConfig,
    provider: &dyn Provider,
    path: &str,
) -> Result<String> {
    healthy(pair, provider).await?;
    let conflict = db
        .conflicts(&pair.id)?
        .into_iter()
        .find(|c| c.path == path)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidConfig,
                "The conflict has already been resolved or is unavailable.",
            )
        })?;
    if db
        .operations(&pair.id)?
        .iter()
        .any(|o| beneath(&o.path, path))
    {
        return Err(Error::new(
            ErrorCode::Conflict,
            "A transfer still targets this path. Pause and let it stop before resolving.",
        ));
    }
    let inventory = remote::snapshot(
        provider,
        &pair.remote_root_id,
        &pair.excludes,
        &db.baselines(&pair.id)?,
    )
    .await?;
    let remote=inventory.entries.get(path).filter(|v|v.len()==1).map(|v|&v[0]).ok_or_else(||Error::new(ErrorCode::Conflict,"The Drive version is unavailable or ambiguous. Reconnect or rename it before resolving."))?;
    if remote.kind != ItemKind::File {
        return Err(Error::new(
            ErrorCode::Unsupported,
            "Keep both currently supports ordinary file content conflicts.",
        ));
    }
    let key = format!("resolution:{}", conflict.id);
    let saved: Option<(String, LocalEntry)> = db.get(&key)?;
    let (preserved, source) = if let Some(saved) = saved {
        saved
    } else {
        let source = local::entry(&pair.local_root, path)?.ok_or_else(|| {
            Error::new(
                ErrorCode::Conflict,
                "The local version is unavailable. Its recovery needs separate review.",
            )
        })?;
        if source.kind != ItemKind::File {
            return Err(Error::new(
                ErrorCode::Unsupported,
                "Keep both currently supports ordinary files.",
            ));
        }
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        let name_path = Path::new(name);
        let stem = name_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(changed)?
            .chars()
            .take(150)
            .collect::<String>();
        let extension = name_path
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| format!(".{s}"))
            .unwrap_or_default();
        let name = format!(
            "{stem} (local conflict {}){extension}",
            conflict.id.chars().take(8).collect::<String>()
        );
        let preserved = if parent.is_empty() {
            name
        } else {
            format!("{parent}/{name}")
        };
        check_local(pair, &preserved, None)?;
        db.set(&key, &(preserved.clone(), source.clone()))?;
        (preserved, source)
    };
    if local::entry(&pair.local_root, path)?.is_some() {
        check_local(pair, path, Some(&source))?;
        move_no_replace(
            &local::safe_join(&pair.local_root, path)?,
            &local::safe_join(&pair.local_root, &preserved)?,
        )?;
    }
    check_local(pair, &preserved, Some(&source))?;
    db.finish_keep_both(&pair.id, path, &conflict.id)?;
    Ok(preserved)
}

pub async fn approve_deletions(
    db: &mut Database,
    pair: &PairConfig,
    provider: &dyn Provider,
    reviewed_count: usize,
) -> Result<()> {
    if !db
        .get::<bool>(&format!("deletion_hold:{}", pair.id))?
        .unwrap_or(false)
    {
        return Err(Error::new(
            ErrorCode::InvalidConfig,
            "This pair has no stopped deletion plan.",
        ));
    }
    healthy(pair, provider).await?;
    let local = local::scan(&pair.local_root, &pair.excludes)?;
    let base = db.baselines(&pair.id)?;
    let remote = remote::snapshot(provider, &pair.remote_root_id, &pair.excludes, &base).await?;
    let fresh = crate::planner::plan(pair, &local, &remote, &base)?;
    let deletes: Vec<_> = db
        .operations(&pair.id)?
        .into_iter()
        .filter(|o| matches!(o.action, Action::TrashRemote | Action::RecycleLocal))
        .collect();
    if reviewed_count != deletes.len() || reviewed_count == 0 {
        return Err(changed());
    }
    let mut approved = vec![];
    for mut original in deletes {
        let current = fresh
            .operations
            .iter()
            .find(|o| o.path == original.path && o.action == original.action)
            .ok_or_else(changed)?;
        if !local_matches(
            current.expected_local.as_ref(),
            original.expected_local.as_ref(),
        ) || match (&current.expected_remote, &original.expected_remote) {
            (Some(a), Some(b)) => {
                a.id != b.id || !a.content_eq(b) || a.parents != b.parents || a.name != b.name
            }
            (None, None) => false,
            _ => true,
        } {
            return Err(changed());
        }
        original.expected_remote = current.expected_remote.clone();
        original.state = OperationState::Prepared;
        original.error = None;
        original.retry_at = 0;
        approved.push(original);
    }
    db.approve_deletion_operations(&pair.id, &approved)
}

pub async fn execute(
    db: &mut Database,
    profile: &Path,
    pair: &PairConfig,
    provider: &dyn Provider,
    cancel: &CancellationToken,
    chunk_size: usize,
) -> Result<usize> {
    if !pair.test_only {
        crate::adoption::authorize(db, pair)?;
    }
    if chunk_size == 0 {
        return Err(Error::new(
            ErrorCode::UnsafePath,
            "Choose a nonzero transfer chunk size.",
        ));
    }
    let queue = db.operations(&pair.id)?;
    let approved = db
        .get::<Vec<String>>(&format!("deletion_approved:{}", pair.id))?
        .unwrap_or_default();
    if queue
        .iter()
        .filter(|o| matches!(o.action, Action::TrashRemote | Action::RecycleLocal))
        .count()
        > pair.deletion_limit
        && !queue
            .iter()
            .filter(|o| matches!(o.action, Action::TrashRemote | Action::RecycleLocal))
            .all(|o| approved.contains(&o.id))
    {
        db.set(&format!("deletion_hold:{}", pair.id), &true)?;
        return Err(Error::new(
            ErrorCode::Conflict,
            "Unexpected large deletion plan stopped. Review the pair and revalidate before resuming.",
        ));
    }
    let mut done = 0;
    for mut op in queue {
        checkpoint(db, cancel)?;
        if op.retry_at > now() {
            continue;
        }
        healthy(pair, provider).await?;
        op.state = OperationState::Running;
        op.error = None;
        op.attempts += 1;
        db.save_operation(&op)?;
        db.set_status(&crate::engine::status(db, pair, "syncing", None)?)?;
        match perform(db, profile, pair, provider, &mut op, cancel, chunk_size).await {
            Ok(()) => done += 1,
            Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
            Err(e)
                if matches!(
                    e.code,
                    ErrorCode::Conflict | ErrorCode::UnsafePath | ErrorCode::Unsupported
                ) =>
            {
                db.conflict_operation(&op, &e.message)?;
            }
            Err(e) if e.code == ErrorCode::IncompleteScan => return Err(e),
            Err(e) => {
                op.state = OperationState::Retry;
                op.retry_at = now()
                    + e.retry_after_secs
                        .unwrap_or_else(|| 2u64.saturating_pow(op.attempts.min(8)))
                        .max(1);
                op.error = Some(e.clone());
                db.save_operation(&op)?;
                if matches!(e.code, ErrorCode::Authentication | ErrorCode::Permission) {
                    return Err(e);
                }
            }
        }
    }
    Ok(done)
}
