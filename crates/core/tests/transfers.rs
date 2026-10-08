use freesync_core::{
    db::Database,
    executor,
    fake::{FakeProvider, Fault},
    local, reconcile, remote, *,
};
use std::fs;
use tokio_util::sync::CancellationToken;
fn pair(root: &std::path::Path) -> PairConfig {
    PairConfig {
        id: "test".into(),
        account_email: "fixture@example.invalid".into(),
        local_root: root.into(),
        remote_root_id: "root".into(),
        remote_root_name: "test-freesync".into(),
        root_identity: local::scan(root, &[]).unwrap().root_identity,
        excludes: vec![],
        enabled: false,
        poll_secs: 1,
        deletion_limit: 20,
        test_only: true,
    }
}
async fn cycle(
    db: &mut Database,
    profile: &std::path::Path,
    pair: &PairConfig,
    cloud: &FakeProvider,
) -> usize {
    reconcile::prepare(db, pair, cloud).await.unwrap();
    executor::execute(
        db,
        profile,
        pair,
        cloud,
        &CancellationToken::new(),
        256 * 1024,
    )
    .await
    .unwrap()
}
#[tokio::test]
async fn bidirectional_zero_byte_and_large_files_have_verified_content_and_stable_baselines() {
    let root = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let cloud = FakeProvider::default();
    let pair = pair(root.path());
    let mut db = Database::open(profile.path()).unwrap();
    db.save_pair(&pair).unwrap();
    let large = vec![27; 1024 * 1024 + 17];
    fs::write(root.path().join("large"), &large).unwrap();
    fs::write(root.path().join("zero"), []).unwrap();
    cloud.seed("root", "download-zero", &[], ItemKind::File);
    cloud.seed("root", "download", b"remote", ItemKind::File);
    assert_eq!(cycle(&mut db, profile.path(), &pair, &cloud).await, 4);
    assert_eq!(fs::read(root.path().join("download")).unwrap(), b"remote");
    assert!(
        fs::read(root.path().join("download-zero"))
            .unwrap()
            .is_empty()
    );
    let base = db.baselines(&pair.id).unwrap();
    let remote = base.iter().find(|b| b.path == "large").unwrap();
    assert_eq!(
        cloud
            .download(&remote.remote.id, 0, large.len())
            .await
            .unwrap(),
        large
    );
    assert_eq!(cycle(&mut db, profile.path(), &pair, &cloud).await, 0);
    assert!(db.operations(&pair.id).unwrap().is_empty());
    assert!(db.conflicts(&pair.id).unwrap().is_empty());
}
#[tokio::test]
async fn upload_interruption_and_uncertain_final_response_resume_without_duplicate_objects() {
    let root = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let cloud = FakeProvider::default();
    let pair = pair(root.path());
    let mut db = Database::open(profile.path()).unwrap();
    db.save_pair(&pair).unwrap();
    fs::write(root.path().join("large"), vec![9; 700_000]).unwrap();
    cloud.inject("upload_chunk", Fault::Pass);
    cloud.inject("upload_chunk", Fault::Timeout);
    assert_eq!(cycle(&mut db, profile.path(), &pair, &cloud).await, 0);
    let mut op = db.operations(&pair.id).unwrap().remove(0);
    assert_eq!(
        op.upload_session.as_ref().unwrap().uploaded_bytes,
        256 * 1024
    );
    let id = op.reserved_remote_id.clone();
    op.retry_at = 0;
    db.save_operation(&op).unwrap();
    drop(db);
    let mut db = Database::open(profile.path()).unwrap();
    cloud.inject("upload_chunk", Fault::Pass);
    cloud.inject("upload_chunk", Fault::UncertainSuccess);
    assert_eq!(cycle(&mut db, profile.path(), &pair, &cloud).await, 0);
    let mut op = db.operations(&pair.id).unwrap().remove(0);
    op.retry_at = 0;
    db.save_operation(&op).unwrap();
    drop(db);
    let mut db = Database::open(profile.path()).unwrap();
    assert_eq!(cycle(&mut db, profile.path(), &pair, &cloud).await, 1);
    assert_eq!(cloud.children("root", None).await.unwrap().items.len(), 1);
    assert_eq!(db.baselines(&pair.id).unwrap()[0].remote.id, id.unwrap());
}
#[tokio::test]
async fn an_intervening_remote_edit_blocks_a_resumed_upload() {
    let root = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let cloud = FakeProvider::default();
    let pair = pair(root.path());
    let mut db = Database::open(profile.path()).unwrap();
    db.save_pair(&pair).unwrap();
    fs::write(root.path().join("file"), b"original").unwrap();
    let id = cloud.seed("root", "file", b"original", ItemKind::File);
    cycle(&mut db, profile.path(), &pair, &cloud).await;
    fs::write(root.path().join("file"), vec![1; 700_000]).unwrap();
    cloud.inject("upload_chunk", Fault::Pass);
    cloud.inject("upload_chunk", Fault::Timeout);
    cycle(&mut db, profile.path(), &pair, &cloud).await;
    cloud.edit(&id, b"intervening remote edit");
    let mut op = db.operations(&pair.id).unwrap().remove(0);
    op.retry_at = 0;
    db.save_operation(&op).unwrap();
    cycle(&mut db, profile.path(), &pair, &cloud).await;
    assert_eq!(
        cloud.download(&id, 0, 100).await.unwrap(),
        b"intervening remote edit"
    );
    assert!(!db.conflicts(&pair.id).unwrap().is_empty());
    assert_eq!(fs::read(root.path().join("file")).unwrap().len(), 700_000);
}
#[tokio::test]
async fn changing_downloads_and_stale_local_sources_preserve_originals() {
    let root = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let cloud = FakeProvider::default();
    let pair = pair(root.path());
    let mut db = Database::open(profile.path()).unwrap();
    db.save_pair(&pair).unwrap();
    fs::write(root.path().join("file"), b"original").unwrap();
    let id = cloud.seed("root", "file", b"original", ItemKind::File);
    cycle(&mut db, profile.path(), &pair, &cloud).await;
    cloud.edit(&id, b"new remote");
    reconcile::prepare(&mut db, &pair, &cloud).await.unwrap();
    cloud.change_during_download(&id, b"changed while downloading".to_vec());
    executor::execute(
        &mut db,
        profile.path(),
        &pair,
        &cloud,
        &CancellationToken::new(),
        256 * 1024,
    )
    .await
    .unwrap();
    assert_eq!(fs::read(root.path().join("file")).unwrap(), b"original");
    assert!(!db.conflicts(&pair.id).unwrap().is_empty());
    fs::write(root.path().join("new"), b"planned").unwrap();
    reconcile::prepare(&mut db, &pair, &cloud).await.unwrap();
    fs::write(root.path().join("new"), b"changed before upload").unwrap();
    executor::execute(
        &mut db,
        profile.path(),
        &pair,
        &cloud,
        &CancellationToken::new(),
        256 * 1024,
    )
    .await
    .unwrap();
    assert!(
        !cloud
            .children("root", None)
            .await
            .unwrap()
            .items
            .iter()
            .any(|i| i.name == "new")
    );
}
#[tokio::test]
async fn replacements_moves_and_deletions_retain_recovery_and_persist_remote_state() {
    let root = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let cloud = FakeProvider::default();
    let pair = pair(root.path());
    let mut db = Database::open(profile.path()).unwrap();
    db.save_pair(&pair).unwrap();
    fs::create_dir(root.path().join("old")).unwrap();
    fs::write(root.path().join("old/file"), b"original").unwrap();
    cycle(&mut db, profile.path(), &pair, &cloud).await;
    fs::rename(root.path().join("old"), root.path().join("new")).unwrap();
    assert_eq!(cycle(&mut db, profile.path(), &pair, &cloud).await, 1);
    assert!(
        !db.baselines(&pair.id)
            .unwrap()
            .iter()
            .any(|b| b.path.starts_with("old"))
    );
    let f = db
        .baselines(&pair.id)
        .unwrap()
        .into_iter()
        .find(|b| b.path == "new/file")
        .unwrap();
    cloud.edit(&f.remote.id, b"replacement");
    assert_eq!(cycle(&mut db, profile.path(), &pair, &cloud).await, 1);
    assert_eq!(
        fs::read(root.path().join("new/file")).unwrap(),
        b"replacement"
    );
    assert!(
        fs::read_dir(profile.path().join("recovery"))
            .unwrap()
            .any(|d| fs::read(d.unwrap().path().join("content")).is_ok_and(|b| b == b"original"))
    );
    let folder = db
        .baselines(&pair.id)
        .unwrap()
        .into_iter()
        .find(|b| b.path == "new")
        .unwrap();
    cloud
        .move_item(&folder.remote, "root", "remote-move")
        .await
        .unwrap();
    cycle(&mut db, profile.path(), &pair, &cloud).await;
    assert!(root.path().join("remote-move/file").exists());
    fs::remove_file(root.path().join("remote-move/file")).unwrap();
    cycle(&mut db, profile.path(), &pair, &cloud).await;
    assert!(cloud.get(&f.remote.id).await.unwrap().trashed);
    let inventory = remote::snapshot(&cloud, "root", &[], &db.baselines(&pair.id).unwrap())
        .await
        .unwrap();
    assert!(inventory.entries.contains_key("remote-move"));
}

#[tokio::test]
async fn keep_both_preserves_local_and_drive_versions_with_a_stable_empty_queue() {
    let root = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let cloud = FakeProvider::default();
    let pair = pair(root.path());
    let mut db = Database::open(profile.path()).unwrap();
    db.save_pair(&pair).unwrap();
    fs::write(root.path().join("file.txt"), b"original").unwrap();
    let id = cloud.seed("root", "file.txt", b"original", ItemKind::File);
    cycle(&mut db, profile.path(), &pair, &cloud).await;
    fs::write(root.path().join("file.txt"), b"local edit").unwrap();
    cloud.edit(&id, b"remote edit");
    cycle(&mut db, profile.path(), &pair, &cloud).await;
    assert_eq!(db.conflicts(&pair.id).unwrap().len(), 1);
    let preserved = executor::keep_both(&mut db, &pair, &cloud, "file.txt")
        .await
        .unwrap();
    assert_eq!(
        fs::read(root.path().join(&preserved)).unwrap(),
        b"local edit"
    );
    cycle(&mut db, profile.path(), &pair, &cloud).await;
    assert_eq!(
        fs::read(root.path().join("file.txt")).unwrap(),
        b"remote edit"
    );
    let copy = db
        .baselines(&pair.id)
        .unwrap()
        .into_iter()
        .find(|b| b.path == preserved)
        .unwrap();
    assert_ne!(copy.remote.id, id);
    assert_eq!(
        cloud.download(&copy.remote.id, 0, 100).await.unwrap(),
        b"local edit"
    );
    assert_eq!(cycle(&mut db, profile.path(), &pair, &cloud).await, 0);
    assert!(db.conflicts(&pair.id).unwrap().is_empty());
    assert!(db.operations(&pair.id).unwrap().is_empty());
}

#[tokio::test]
async fn metadata_only_source_version_bumps_allow_verified_downloads_but_not_stale_writes() {
    let root = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let cloud = FakeProvider::default();
    let pair = pair(root.path());
    let mut db = Database::open(profile.path()).unwrap();
    db.save_pair(&pair).unwrap();
    let id = cloud.seed("root", "file", b"original", ItemKind::File);
    cycle(&mut db, profile.path(), &pair, &cloud).await;
    cloud.edit(&id, b"new remote");
    reconcile::prepare(&mut db, &pair, &cloud).await.unwrap();
    let old = cloud.get(&id).await.unwrap();
    cloud.move_item(&old, "root", "file").await.unwrap();
    executor::execute(
        &mut db,
        profile.path(),
        &pair,
        &cloud,
        &CancellationToken::new(),
        256 * 1024,
    )
    .await
    .unwrap();
    assert_eq!(fs::read(root.path().join("file")).unwrap(), b"new remote");
    assert!(db.conflicts(&pair.id).unwrap().is_empty());
    fs::write(root.path().join("file"), b"new local").unwrap();
    reconcile::prepare(&mut db, &pair, &cloud).await.unwrap();
    let old = cloud.get(&id).await.unwrap();
    cloud.move_item(&old, "root", "file").await.unwrap();
    executor::execute(
        &mut db,
        profile.path(),
        &pair,
        &cloud,
        &CancellationToken::new(),
        256 * 1024,
    )
    .await
    .unwrap();
    assert_eq!(cloud.download(&id, 0, 100).await.unwrap(), b"new remote");
    assert_eq!(db.conflicts(&pair.id).unwrap().len(), 1);
}
