use async_trait::async_trait;
use freesync_core::{
    db::Database,
    engine::{self, ProviderFactory},
    fake::{FakeProvider, Fault},
    local,
    profile::ProfileLock,
    *,
};
use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
struct Factory(Arc<FakeProvider>);
#[async_trait]
impl ProviderFactory for Factory {
    async fn connect(&self, _: &PairConfig) -> Result<Arc<dyn Provider>> {
        Ok(self.0.clone())
    }
}
fn setup() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    PairConfig,
    Arc<FakeProvider>,
) {
    let root = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let pair = PairConfig {
        id: "test".into(),
        account_email: "fixture@example.invalid".into(),
        local_root: root.path().into(),
        remote_root_id: "root".into(),
        remote_root_name: "test-freesync".into(),
        root_identity: local::scan(root.path(), &[]).unwrap().root_identity,
        excludes: vec![],
        enabled: true,
        poll_secs: 1,
        deletion_limit: 10,
        test_only: true,
    };
    Database::open(profile.path())
        .unwrap()
        .save_pair(&pair)
        .unwrap();
    (root, profile, pair, Arc::new(FakeProvider::default()))
}
fn start(
    profile: PathBuf,
    cloud: Arc<FakeProvider>,
    cancel: CancellationToken,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let _owner = ProfileLock::acquire(&profile).unwrap();
        let mut db = Database::open(&profile).unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(engine::run(&mut db, &profile, &Factory(cloud), cancel))
            .unwrap();
    })
}
async fn wait(profile: &std::path::Path, predicate: impl Fn(&Database) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let db = Database::open(profile).unwrap();
        if predicate(&db) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    panic!("engine state did not converge before timeout");
}
#[tokio::test]
async fn native_changes_pause_resume_and_restart_keep_one_owner_and_a_stable_queue() {
    let (root, profile, pair, cloud) = setup();
    let cancel = CancellationToken::new();
    let worker = start(profile.path().into(), cloud.clone(), cancel.clone());
    wait(profile.path(), |db| {
        db.status(&pair.id)
            .unwrap()
            .is_some_and(|s| s.state == "idle")
    })
    .await;
    assert_eq!(
        ProfileLock::acquire(profile.path()).err().unwrap().code,
        ErrorCode::LockBusy
    );
    fs::write(root.path().join("file"), b"native event").unwrap();
    wait(profile.path(), |db| {
        db.baselines(&pair.id)
            .unwrap()
            .iter()
            .any(|b| b.path == "file")
    })
    .await;
    let db = Database::open(profile.path()).unwrap();
    let mut controls = db.controls().unwrap();
    controls.paused = true;
    db.set("controls", &controls).unwrap();
    wait(profile.path(), |db| {
        db.status(&pair.id)
            .unwrap()
            .is_some_and(|s| s.state == "paused")
    })
    .await;
    let old = db.baselines(&pair.id).unwrap()[0].remote.clone();
    fs::write(root.path().join("file"), b"during pause").unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        cloud.download(&old.id, 0, 100).await.unwrap(),
        b"native event"
    );
    controls.paused = false;
    controls.sync_now += 1;
    db.set("controls", &controls).unwrap();
    wait(profile.path(), |db| {
        db.baselines(&pair.id).unwrap()[0]
            .local
            .fingerprint
            .as_ref()
            .unwrap()
            .size
            == 12
    })
    .await;
    cancel.cancel();
    worker.join().unwrap();
    assert_eq!(db.status(&pair.id).unwrap().unwrap().state, "stopped");
    fs::write(root.path().join("restart"), b"offline on disk").unwrap();
    let next = CancellationToken::new();
    let worker = start(profile.path().into(), cloud.clone(), next.clone());
    wait(profile.path(), |db| {
        db.baselines(&pair.id).unwrap().len() == 2 && db.operations(&pair.id).unwrap().is_empty()
    })
    .await;
    next.cancel();
    worker.join().unwrap();
    assert!(ProfileLock::acquire(profile.path()).is_ok());
}
#[tokio::test]
async fn offline_failures_and_conflicts_do_not_discard_work_or_stall_other_paths() {
    let (root, profile, pair, cloud) = setup();
    let mut db = Database::open(profile.path()).unwrap();
    let factory = Factory(cloud.clone());
    let cancel = CancellationToken::new();
    fs::write(root.path().join("file"), b"original").unwrap();
    cloud.inject("get", Fault::Timeout);
    assert!(
        engine::cycle(&mut db, profile.path(), &pair, &factory, &cancel)
            .await
            .is_err()
    );
    assert!(
        db.get::<LocalInventory>("offline_local:test")
            .unwrap()
            .is_some()
    );
    engine::cycle(&mut db, profile.path(), &pair, &factory, &cancel)
        .await
        .unwrap();
    let id = db.baselines(&pair.id).unwrap()[0].remote.id.clone();
    fs::write(root.path().join("file"), b"local edit").unwrap();
    cloud.edit(&id, b"remote edit");
    engine::cycle(&mut db, profile.path(), &pair, &factory, &cancel)
        .await
        .unwrap();
    assert_eq!(db.conflicts(&pair.id).unwrap().len(), 1);
    fs::write(root.path().join("other"), b"unrelated change").unwrap();
    engine::cycle(&mut db, profile.path(), &pair, &factory, &cancel)
        .await
        .unwrap();
    assert!(
        db.baselines(&pair.id)
            .unwrap()
            .iter()
            .any(|b| b.path == "other")
    );
    assert_eq!(db.conflicts(&pair.id).unwrap().len(), 1);
    assert!(db.operations(&pair.id).unwrap().is_empty());
}

#[tokio::test]
async fn oversized_deletions_require_reviewed_count_and_fresh_unchanged_content() {
    let (root, profile, mut pair, cloud) = setup();
    pair.deletion_limit = 1;
    let mut db = Database::open(profile.path()).unwrap();
    db.save_pair(&pair).unwrap();
    let factory = Factory(cloud.clone());
    let cancel = CancellationToken::new();
    fs::write(root.path().join("a"), b"original").unwrap();
    fs::write(root.path().join("b"), b"original").unwrap();
    engine::cycle(&mut db, profile.path(), &pair, &factory, &cancel)
        .await
        .unwrap();
    let base = db.baselines(&pair.id).unwrap();
    fs::remove_file(root.path().join("a")).unwrap();
    fs::remove_file(root.path().join("b")).unwrap();
    assert!(
        engine::cycle(&mut db, profile.path(), &pair, &factory, &cancel)
            .await
            .is_err()
    );
    assert!(db.get::<bool>("deletion_hold:test").unwrap().unwrap());
    assert!(!cloud.get(&base[0].remote.id).await.unwrap().trashed);
    assert!(
        freesync_core::executor::approve_deletions(&mut db, &pair, cloud.as_ref(), 1)
            .await
            .is_err()
    );
    cloud.edit(&base[0].remote.id, b"intervening edit");
    assert!(
        freesync_core::executor::approve_deletions(&mut db, &pair, cloud.as_ref(), 2)
            .await
            .is_err()
    );
    assert!(db.get::<bool>("deletion_hold:test").unwrap().unwrap());
    cloud.edit(&base[0].remote.id, b"original");
    freesync_core::executor::approve_deletions(&mut db, &pair, cloud.as_ref(), 2)
        .await
        .unwrap();
    engine::cycle(&mut db, profile.path(), &pair, &factory, &cancel)
        .await
        .unwrap();
    assert!(cloud.get(&base[0].remote.id).await.unwrap().trashed);
    assert!(db.operations(&pair.id).unwrap().is_empty());
}

#[tokio::test]
async fn desktop_requests_are_serialized_on_the_owner_and_preview_holds_transfers() {
    let (root, profile, mut pair, cloud) = setup();
    pair.enabled = false;
    Database::open(profile.path())
        .unwrap()
        .save_pair(&pair)
        .unwrap();
    let cancel = CancellationToken::new();
    let (tx, rx) = tokio::sync::mpsc::channel(4);
    let location = profile.path().to_path_buf();
    let token = cancel.clone();
    let worker = std::thread::spawn(move || {
        let _owner = ProfileLock::acquire(&location).unwrap();
        let mut db = Database::open(&location).unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(engine::run_controlled(
                &mut db,
                &location,
                &Factory(cloud),
                token,
                rx,
            ))
            .unwrap();
    });
    fs::write(root.path().join("reviewed"), b"review first").unwrap();
    let (reply, response) = tokio::sync::oneshot::channel();
    tx.send(engine::EngineRequest {
        command: engine::EngineCommand::Preview {
            pair_id: pair.id.clone(),
        },
        reply,
    })
    .await
    .unwrap();
    let preview = response.await.unwrap().unwrap();
    assert_eq!(preview["counts"]["operations"], 1);
    let db = Database::open(profile.path()).unwrap();
    assert!(db.controls().unwrap().paused);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(db.baselines(&pair.id).unwrap().is_empty());
    assert_eq!(
        ProfileLock::acquire(profile.path()).err().unwrap().code,
        ErrorCode::LockBusy
    );
    let (reply, response) = tokio::sync::oneshot::channel();
    tx.send(engine::EngineRequest {
        command: engine::EngineCommand::Activate {
            pair_id: pair.id.clone(),
        },
        reply,
    })
    .await
    .unwrap();
    assert_eq!(response.await.unwrap().unwrap()["activated"], true);
    wait(profile.path(), |db| {
        db.baselines(&pair.id).unwrap().len() == 1 && db.operations(&pair.id).unwrap().is_empty()
    })
    .await;
    cancel.cancel();
    worker.join().unwrap();
    assert!(ProfileLock::acquire(profile.path()).is_ok());
}
