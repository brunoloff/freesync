use freesync_core::{
    adoption::{Manifest, Source},
    fake::{FakeProvider, Fault},
    local, *,
};
use tokio_util::sync::CancellationToken;
fn fixture() -> (tempfile::TempDir, Source, Manifest, FakeProvider) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("files");
    std::fs::create_dir(&root).unwrap();
    let source = Source {
        account_email: "fixture@example.invalid".into(),
        local_root: root.clone(),
        remote_root_id: "root".into(),
        root_identity: local::identity(&std::fs::metadata(root).unwrap()).unwrap(),
        excludes: vec!["ignored".into()],
        exclusion_source: "Fixture confirmed rules".into(),
    };
    let m = Manifest::open(&tmp.path().join("manifest")).unwrap();
    m.initialize(&source).unwrap();
    (tmp, source, m, FakeProvider::new(1))
}
#[tokio::test]
async fn changing_exclusions_preserves_inventory_but_invalidates_review() {
    let (_t, mut source, manifest, provider) = fixture();
    std::fs::create_dir(source.local_root.join("scope")).unwrap();
    provider.seed("root", "scope", b"", ItemKind::Folder);
    manifest
        .run(&provider, &CancellationToken::new())
        .await
        .unwrap();
    let old = manifest.report(None, "", 0, 10).unwrap();
    manifest.scope("scope", old.progress.revision).unwrap();
    source.excludes.push("scope".into());
    manifest.initialize(&source).unwrap();
    let updated = manifest.report(None, "", 0, 10).unwrap();
    assert_eq!(updated.progress.phase, "pending");
    assert!(updated.findings.is_empty());
    assert_eq!(updated.progress.remote_items, old.progress.remote_items);
    assert_eq!(manifest.get::<bool>("remote_done").unwrap(), Some(true));
    assert!(manifest.scope("scope", old.progress.revision).is_err());
    manifest
        .run(&provider, &CancellationToken::new())
        .await
        .unwrap();
    let report = manifest.report(None, "", 0, 10).unwrap();
    assert_eq!(report.counts["excluded"], 1);
    assert!(report.progress.revision > old.progress.revision);
}
#[tokio::test]
async fn adopts_without_transfers_and_isolates_discrepancies() {
    let (_t, s, m, p) = fixture();
    std::fs::write(s.local_root.join("same.txt"), b"same").unwrap();
    let id = p.seed("root", "same.txt", b"same", ItemKind::File);
    std::fs::write(s.local_root.join("changed.txt"), b"local").unwrap();
    p.seed("root", "changed.txt", b"remote", ItemKind::File);
    std::fs::write(s.local_root.join("local.txt"), b"local").unwrap();
    p.seed("root", "remote.txt", b"remote", ItemKind::File);
    p.seed("root", "duplicate", b"1", ItemKind::File);
    p.seed("root", "duplicate", b"2", ItemKind::File);
    std::fs::create_dir(s.local_root.join("ignored")).unwrap();
    std::fs::write(s.local_root.join("ignored/private"), b"ignore").unwrap();
    p.seed("root", "ignored", b"", ItemKind::Folder);
    m.run(&p, &CancellationToken::new()).await.unwrap();
    let report = m.report(None, "", 0, 100).unwrap();
    assert_eq!(report.progress.phase, "ready");
    assert_eq!(report.counts["matched"], 1);
    assert_eq!(report.counts["unresolved"], 2);
    assert_eq!(report.counts["upload"], 1);
    assert_eq!(report.counts["download"], 1);
    assert_eq!(report.counts["excluded"], 1);
    assert_eq!(
        report
            .findings
            .iter()
            .find(|f| f.path == "same.txt")
            .unwrap()
            .remote_id
            .as_ref(),
        Some(&id)
    );
    assert_eq!(p.get(&id).await.unwrap().version, "1");
    assert!(!s.local_root.join("remote.txt").exists());
}
#[tokio::test]
async fn resumes_remote_checkpoint_and_rechecks_changed_local_content() {
    let (_t, s, m, p) = fixture();
    std::fs::write(s.local_root.join("one"), b"one").unwrap();
    let id = p.seed("root", "one", b"one", ItemKind::File);
    p.seed("root", "two", b"two", ItemKind::File);
    p.inject("inventory", Fault::Pass);
    p.inject("inventory", Fault::Timeout);
    assert_eq!(
        m.run(&p, &CancellationToken::new()).await.unwrap_err().code,
        ErrorCode::Transient
    );
    let interrupted = m.report(None, "", 0, 10).unwrap().progress;
    assert_eq!(interrupted.remote_items, 1);
    assert_eq!(interrupted.phase, "failed");
    assert!(interrupted.current_path.is_none());
    m.run(&p, &CancellationToken::new()).await.unwrap();
    assert_eq!(m.report(None, "", 0, 10).unwrap().counts["matched"], 1);
    std::fs::write(s.local_root.join("one"), b"changed size").unwrap();
    m.run(&p, &CancellationToken::new()).await.unwrap();
    let report = m.report(None, "", 0, 10).unwrap();
    assert_eq!(report.counts["unresolved"], 1);
    assert!(!report.counts.contains_key("matched"));
    assert_eq!(p.get(&id).await.unwrap().version, "1");
}
#[tokio::test]
async fn cancellation_and_account_root_guards() {
    let (_t, mut s, m, p) = fixture();
    let c = CancellationToken::new();
    c.cancel();
    assert_eq!(m.run(&p, &c).await.unwrap_err().code, ErrorCode::Cancelled);
    m.run(&p, &CancellationToken::new()).await.unwrap();
    s.account_email = "wrong@example.invalid".into();
    assert_eq!(m.initialize(&s).unwrap_err().code, ErrorCode::Conflict);
    std::fs::rename(&s.local_root, s.local_root.with_extension("old")).unwrap();
    std::fs::create_dir(&s.local_root).unwrap();
    assert_eq!(
        m.run(&p, &CancellationToken::new()).await.unwrap_err().code,
        ErrorCode::UnsafePath
    );
}
#[tokio::test]
async fn scope_requires_finished_unique_folder_and_review_revision() {
    let (_t, s, m, p) = fixture();
    std::fs::create_dir(s.local_root.join("selected")).unwrap();
    p.seed("root", "selected", b"", ItemKind::Folder);
    m.run(&p, &CancellationToken::new()).await.unwrap();
    let r = m.report(None, "", 0, 10).unwrap();
    let pair = m.scope("selected", r.progress.revision).unwrap();
    assert!(!pair.enabled);
    assert!(!pair.test_only);
    assert_eq!(pair.local_root, s.local_root.join("selected"));
    assert_eq!(
        m.scope("selected", r.progress.revision + 1)
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert!(m.scope("../selected", r.progress.revision).is_err());
}
#[tokio::test]
async fn moving_an_ancestor_rejects_activation_even_when_selected_contents_are_unchanged() {
    let (_t, source, manifest, provider) = fixture();
    std::fs::create_dir_all(source.local_root.join("parent/scope")).unwrap();
    let parent = provider.seed("root", "parent", b"", ItemKind::Folder);
    provider.seed(&parent, "scope", b"", ItemKind::Folder);
    let elsewhere = provider.seed("root", "elsewhere", b"", ItemKind::Folder);
    manifest
        .run(&provider, &CancellationToken::new())
        .await
        .unwrap();
    manifest
        .verify_remote_location(&provider, "parent/scope")
        .await
        .unwrap();
    provider
        .move_item(&provider.get(&parent).await.unwrap(), &elsewhere, "parent")
        .await
        .unwrap();
    assert_eq!(
        manifest
            .verify_remote_location(&provider, "parent/scope")
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
}
#[tokio::test]
async fn unicode_scope_rechecks_and_imported_exclusions_remain_protected() {
    let (_t, mut s, m, p) = fixture();
    let name = "Meditação [notes]";
    s.excludes.push(format!(
        "{}/future-private",
        freesync_core::adoption::literal_glob(name)
    ));
    m.initialize(&s).unwrap();
    std::fs::create_dir(s.local_root.join(name)).unwrap();
    let folder = p.seed("root", name, b"", ItemKind::Folder);
    std::fs::write(s.local_root.join(name).join("same"), b"same").unwrap();
    p.seed(&folder, "same", b"same", ItemKind::File);
    std::fs::write(s.local_root.join(name).join("Link.gdoc"), b"native link").unwrap();
    m.run(&p, &CancellationToken::new()).await.unwrap();
    let r = m.report(None, "", 0, 100).unwrap();
    let pair = m.scope(name, r.progress.revision).unwrap();
    assert!(
        local::Exclusions::new(&pair.excludes)
            .unwrap()
            .excludes("future-private/new-file")
    );
    assert_eq!(
        m.review_scope(name, r.progress.revision).unwrap()["counts"]["matched"],
        1
    );
    let l = local::scan(&pair.local_root, &pair.excludes).unwrap();
    let remote = freesync_core::remote::snapshot(&p, &pair.remote_root_id, &pair.excludes, &[])
        .await
        .unwrap();
    m.verify_scope(name, &pair, &l, &remote).unwrap();
    assert_eq!(l.entries.len(), 1);
    let same_id = remote.entries["same"][0].id.clone();
    p.edit(&same_id, b"same");
    let refreshed = freesync_core::remote::snapshot(&p, &pair.remote_root_id, &pair.excludes, &[])
        .await
        .unwrap();
    m.verify_scope(name, &pair, &l, &refreshed).unwrap();
    std::fs::write(pair.local_root.join("same"), b"stale").unwrap();
    let l = local::scan(&pair.local_root, &pair.excludes).unwrap();
    assert_eq!(
        m.verify_scope(name, &pair, &l, &remote).unwrap_err().code,
        ErrorCode::Conflict
    );
}
#[tokio::test]
async fn a_glob_crossing_the_scope_ancestors_cannot_be_silently_dropped() {
    let (_t, mut s, m, p) = fixture();
    s.excludes.push("*/future-private".into());
    m.initialize(&s).unwrap();
    std::fs::create_dir(s.local_root.join("scope")).unwrap();
    p.seed("root", "scope", b"", ItemKind::Folder);
    m.run(&p, &CancellationToken::new()).await.unwrap();
    let report = m.report(None, "", 0, 10).unwrap();
    assert_eq!(
        m.scope("scope", report.progress.revision).unwrap_err().code,
        ErrorCode::Unsupported
    );
}
#[tokio::test]
async fn approval_binds_root_exclusions_and_preserves_mismatches_through_activation() {
    let (t, s, m, p) = fixture();
    std::fs::create_dir(s.local_root.join("scope")).unwrap();
    let folder = p.seed("root", "scope", b"", ItemKind::Folder);
    std::fs::write(s.local_root.join("scope/same"), b"same").unwrap();
    let same = p.seed(&folder, "same", b"same", ItemKind::File);
    std::fs::write(s.local_root.join("scope/different"), b"local").unwrap();
    p.seed(&folder, "different", b"drive", ItemKind::File);
    m.run(&p, &CancellationToken::new()).await.unwrap();
    let report = m.report(None, "", 0, 100).unwrap();
    let mut pair = m.scope("scope", report.progress.revision).unwrap();
    let directory = t.path().join("profile");
    let mut db = freesync_core::db::Database::open(&directory).unwrap();
    assert_eq!(
        freesync_core::executor::execute(
            &mut db,
            &directory,
            &pair,
            &p,
            &CancellationToken::new(),
            1024
        )
        .await
        .unwrap_err()
        .code,
        ErrorCode::Permission
    );
    let l = local::scan(&pair.local_root, &pair.excludes).unwrap();
    let remote = freesync_core::remote::snapshot(&p, &folder, &pair.excludes, &[])
        .await
        .unwrap();
    m.verify_scope("scope", &pair, &l, &remote).unwrap();
    let plan = freesync_core::planner::plan(&pair, &l, &remote, &[]).unwrap();
    assert_eq!(plan.accepted.len(), 1);
    assert_eq!(plan.conflicts.len(), 1);
    assert!(plan.operations.is_empty());
    db.backup(&directory.join("backup")).unwrap();
    m.backup(&directory.join("backup")).unwrap();
    pair.enabled = true;
    let grant = freesync_core::adoption::Authorization {
        pair: pair.clone(),
        scope: "scope".into(),
        remote_root: p.get(&folder).await.unwrap(),
        manifest_revision: report.progress.revision,
        approved_at: 1,
    };
    db.adopt(&grant, &plan, &l, &remote).unwrap();
    assert_eq!(db.baselines(&pair.id).unwrap()[0].remote.id, same);
    assert_eq!(db.conflicts(&pair.id).unwrap().len(), 1);
    assert_eq!(
        freesync_core::executor::execute(
            &mut db,
            &directory,
            &pair,
            &p,
            &CancellationToken::new(),
            1024
        )
        .await
        .unwrap(),
        0
    );
    let mut tampered = pair.clone();
    tampered.excludes.clear();
    assert_eq!(
        freesync_core::adoption::authorize(&db, &tampered)
            .unwrap_err()
            .code,
        ErrorCode::UnsafePath
    );
    let backup = freesync_core::db::Database::open(&directory.join("backup")).unwrap();
    assert!(backup.pairs().unwrap().is_empty());
    let restored = Manifest::open(&directory.join("backup")).unwrap();
    assert_eq!(
        restored.report(None, "", 0, 100).unwrap().counts,
        report.counts
    );
}
#[tokio::test]
async fn a_changing_listing_is_repeated_and_an_expired_cursor_keeps_local_checkpoints() {
    let (_t, s, m, p) = fixture();
    std::fs::write(s.local_root.join("one"), b"one").unwrap();
    let id = p.seed("root", "one", b"one", ItemKind::File);
    p.seed("root", "two", b"two", ItemKind::File);
    p.change_after_listing_calls(2, &id, b"remote changed".to_vec());
    m.run(&p, &CancellationToken::new()).await.unwrap();
    let r = m.report(None, "", 0, 100).unwrap();
    assert_eq!(r.progress.drive_passes, 2);
    assert_eq!(r.counts["unresolved"], 1);
    std::fs::write(s.local_root.join("three-local"), b"checkpoint").unwrap();
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(s.local_root.join("three-local"))
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH - std::time::Duration::from_secs(10)),
        )
        .unwrap();
    p.seed("root", "three-local", b"checkpoint", ItemKind::File);
    p.inject("changes", Fault::CursorExpired);
    assert_eq!(
        m.run(&p, &CancellationToken::new()).await.unwrap_err().code,
        ErrorCode::IncompleteScan
    );
    let failed = m.report(None, "", 0, 100).unwrap();
    assert_eq!(failed.progress.local_items, 2);
    assert_eq!(failed.progress.hashed_bytes, r.progress.hashed_bytes + 10);
    assert_eq!(failed.progress.reused_hashes, 1);
    m.run(&p, &CancellationToken::new()).await.unwrap();
    let r = m.report(None, "", 0, 100).unwrap();
    assert_eq!(r.progress.phase, "ready");
    assert!(r.progress.reused_hashes >= 2);
    assert_eq!(
        r.findings
            .iter()
            .find(|f| f.path == "one")
            .unwrap()
            .remote_id
            .as_ref(),
        Some(&id)
    );
}
#[tokio::test]
async fn cancelling_during_local_hashing_preserves_verified_entries() {
    let (_t, s, m, p) = fixture();
    for n in 0..320 {
        let name = format!("small-{n}");
        std::fs::write(s.local_root.join(&name), b"saved").unwrap();
        p.seed("root", &name, b"saved", ItemKind::File);
    }
    let file = std::fs::File::create(s.local_root.join("large")).unwrap();
    file.set_len(128 * 1024 * 1024).unwrap();
    drop(file);
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let directory = m.directory.clone();
    let controller = std::thread::spawn(move || {
        let reader = Manifest::open(&directory).unwrap();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let progress = reader
                .get::<freesync_core::adoption::Progress>("progress")
                .unwrap()
                .unwrap();
            if progress.local_items >= 256 && progress.phase == "local_inventory" {
                token.cancel();
                break;
            }
            assert!(
                std::time::Instant::now() < end,
                "local checkpoint was never observed"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });
    assert_eq!(
        m.run(&p, &cancel).await.unwrap_err().code,
        ErrorCode::Cancelled
    );
    controller.join().unwrap();
    let saved = m.report(None, "", 0, 10).unwrap().progress.local_items;
    assert!(saved >= 256);
    m.run(&p, &CancellationToken::new()).await.unwrap();
    let report = m.report(None, "", 0, 10).unwrap();
    assert!(report.progress.reused_hashes >= saved);
    assert_eq!(report.counts["matched"], 320);
    assert_eq!(report.counts["upload"], 1);
}
