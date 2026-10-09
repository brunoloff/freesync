use async_trait::async_trait;
use freesync_core::{
    conflicts,
    db::Database,
    engine::{self, ProviderFactory},
    executor,
    fake::FakeProvider,
    local,
    profile::ProfileLock,
    reconcile, *,
};
use std::{
    fs,
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
struct Fixture {
    root: tempfile::TempDir,
    profile: tempfile::TempDir,
    pair: PairConfig,
    cloud: Arc<FakeProvider>,
}
async fn fixture(names: &[&str]) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    let cloud = Arc::new(FakeProvider::default());
    let pair = PairConfig {
        id: "test".into(),
        account_email: "fixture@example.invalid".into(),
        local_root: root.path().into(),
        remote_root_id: "root".into(),
        remote_root_name: "test-freesync".into(),
        root_identity: local::scan(root.path(), &[]).unwrap().root_identity,
        excludes: vec![],
        respect_gitignore: true,
        enabled: false,
        poll_secs: 1,
        deletion_limit: 20,
        test_only: true,
    };
    let mut db = Database::open(profile.path()).unwrap();
    db.save_pair(&pair).unwrap();
    let mut ids = vec![];
    for name in names {
        fs::write(root.path().join(name), b"original").unwrap();
        ids.push(cloud.seed("root", name, b"original", ItemKind::File));
    }
    reconcile::prepare(&mut db, &pair, cloud.as_ref())
        .await
        .unwrap();
    for (name, id) in names.iter().zip(ids) {
        fs::write(root.path().join(name), b"local edit").unwrap();
        cloud.edit(&id, b"Drive edit");
    }
    reconcile::prepare(&mut db, &pair, cloud.as_ref())
        .await
        .unwrap();
    Fixture {
        root,
        profile,
        pair,
        cloud,
    }
}
fn enqueue(db: &mut Database, path: &str, choice: ConflictChoice) -> ConflictTask {
    let conflict = db
        .conflicts("test")
        .unwrap()
        .into_iter()
        .find(|c| c.path == path)
        .unwrap();
    let task = ConflictTask {
        id: uuid::Uuid::new_v4().to_string(),
        conflict,
        choice,
        state: "queued".into(),
        error: None,
        operation_id: None,
        preserved_path: None,
        comparison_directory: None,
    };
    db.enqueue_conflict_task(&task).unwrap();
    task
}
async fn process(f: &Fixture, db: &mut Database) {
    conflicts::process_tasks(db, f.profile.path(), &Factory(f.cloud.clone()))
        .await
        .unwrap();
}
async fn execute(f: &Fixture, db: &mut Database) {
    executor::execute(
        db,
        f.profile.path(),
        &f.pair,
        f.cloud.as_ref(),
        &CancellationToken::new(),
        1024,
    )
    .await
    .unwrap();
    process(f, db).await;
}
#[tokio::test]
async fn use_local_and_use_drive_preserve_displaced_bytes_and_finish_durable_jobs() {
    let f = fixture(&["local.txt", "drive.txt"]).await;
    let mut db = Database::open(f.profile.path()).unwrap();
    let local = enqueue(&mut db, "local.txt", ConflictChoice::UseLocal);
    let drive = enqueue(&mut db, "drive.txt", ConflictChoice::UseDrive);
    process(&f, &mut db).await;
    assert_eq!(db.operations("test").unwrap().len(), 2);
    assert!(db.conflicts("test").unwrap().is_empty());
    assert_eq!(
        fs::read(
            f.profile
                .path()
                .join("recovery")
                .join(&local.id)
                .join("drive-original")
        )
        .unwrap(),
        b"Drive edit"
    );
    drop(db);
    let mut db = Database::open(f.profile.path()).unwrap();
    execute(&f, &mut db).await;
    assert!(
        db.conflict_tasks()
            .unwrap()
            .iter()
            .all(|t| t.state == "done")
    );
    assert_eq!(
        fs::read(f.root.path().join("drive.txt")).unwrap(),
        b"Drive edit"
    );
    assert_eq!(
        fs::read(
            f.profile
                .path()
                .join("recovery")
                .join(&drive.id)
                .join("content")
        )
        .unwrap(),
        b"local edit"
    );
    let baseline = db
        .baselines("test")
        .unwrap()
        .into_iter()
        .find(|b| b.path == "local.txt")
        .unwrap();
    assert_eq!(
        f.cloud.download(&baseline.remote.id, 0, 100).await.unwrap(),
        b"local edit"
    );
    assert!(db.operations("test").unwrap().is_empty());
}
#[tokio::test]
async fn stale_choices_reject_overwrite_then_refresh_captures_the_new_versions() {
    let f = fixture(&["file.txt"]).await;
    let mut db = Database::open(f.profile.path()).unwrap();
    let task = enqueue(&mut db, "file.txt", ConflictChoice::UseDrive);
    fs::write(f.root.path().join("file.txt"), b"later local edit").unwrap();
    process(&f, &mut db).await;
    let failed = db
        .conflict_tasks()
        .unwrap()
        .into_iter()
        .find(|t| t.id == task.id)
        .unwrap();
    assert_eq!(failed.state, "failed");
    assert_eq!(failed.error.unwrap().code, ErrorCode::Conflict);
    assert_eq!(
        fs::read(f.root.path().join("file.txt")).unwrap(),
        b"later local edit"
    );
    assert!(db.operations("test").unwrap().is_empty());
    let old_id = task.conflict.id;
    enqueue(&mut db, "file.txt", ConflictChoice::Refresh);
    process(&f, &mut db).await;
    assert_ne!(db.conflicts("test").unwrap()[0].id, old_id);
    enqueue(&mut db, "file.txt", ConflictChoice::UseDrive);
    process(&f, &mut db).await;
    execute(&f, &mut db).await;
    assert_eq!(
        fs::read(f.root.path().join("file.txt")).unwrap(),
        b"Drive edit"
    );
}
#[tokio::test]
async fn a_remote_edit_after_the_choice_is_queued_is_preserved() {
    let f = fixture(&["file.txt"]).await;
    let mut db = Database::open(f.profile.path()).unwrap();
    let task = enqueue(&mut db, "file.txt", ConflictChoice::UseLocal);
    f.cloud.edit(
        &task.conflict.remote.as_ref().unwrap().id,
        b"later Drive edit",
    );
    process(&f, &mut db).await;
    assert_eq!(db.conflict_tasks().unwrap()[0].state, "failed");
    assert!(db.operations("test").unwrap().is_empty());
    assert_eq!(
        f.cloud
            .download(&task.conflict.remote.unwrap().id, 0, 100)
            .await
            .unwrap(),
        b"later Drive edit"
    );
    assert_eq!(
        fs::read(f.root.path().join("file.txt")).unwrap(),
        b"local edit"
    );
}
#[tokio::test]
async fn text_comparison_uses_readonly_snapshots_and_binary_content_never_resolves_the_conflict() {
    let f = fixture(&["file.txt"]).await;
    let mut db = Database::open(f.profile.path()).unwrap();
    enqueue(&mut db, "file.txt", ConflictChoice::Compare);
    process(&f, &mut db).await;
    let task = db.conflict_tasks().unwrap().remove(0);
    assert_eq!(task.state, "ready_to_open");
    let directory = task.comparison_directory.unwrap();
    assert_eq!(
        fs::read(directory.join("local.txt")).unwrap(),
        b"local edit"
    );
    assert_eq!(
        fs::read(directory.join("drive.txt")).unwrap(),
        b"Drive edit"
    );
    assert!(
        fs::metadata(directory.join("drive.txt"))
            .unwrap()
            .permissions()
            .readonly()
    );
    assert_eq!(db.conflicts("test").unwrap().len(), 1);
    let f = fixture(&["binary.bin"]).await;
    let mut db = Database::open(f.profile.path()).unwrap();
    fs::write(f.root.path().join("binary.bin"), [0, 255, 2]).unwrap();
    enqueue(&mut db, "binary.bin", ConflictChoice::Refresh);
    process(&f, &mut db).await;
    enqueue(&mut db, "binary.bin", ConflictChoice::Compare);
    process(&f, &mut db).await;
    let task = db.conflict_tasks().unwrap().pop().unwrap();
    assert_eq!(task.state, "failed");
    assert!(!task.comparison_directory.as_ref().unwrap().exists());
    assert_eq!(task.error.unwrap().code, ErrorCode::Unsupported);
    assert_eq!(
        fs::read(f.root.path().join("binary.bin")).unwrap(),
        [0, 255, 2]
    );
    assert_eq!(db.conflicts("test").unwrap().len(), 1);
}
#[tokio::test]
async fn keep_both_does_not_pause_other_work_and_recovers_a_working_job_after_restart() {
    let f = fixture(&["file.txt"]).await;
    let mut db = Database::open(f.profile.path()).unwrap();
    let mut task = enqueue(&mut db, "file.txt", ConflictChoice::KeepBoth);
    task.state = "working".into();
    db.save_conflict_task(&task).unwrap();
    drop(db);
    let mut db = Database::open(f.profile.path()).unwrap();
    process(&f, &mut db).await;
    assert!(!db.controls().unwrap().paused);
    reconcile::prepare(&mut db, &f.pair, f.cloud.as_ref())
        .await
        .unwrap();
    execute(&f, &mut db).await;
    let task = db.conflict_tasks().unwrap().remove(0);
    assert_eq!(task.state, "done");
    assert_eq!(
        fs::read(f.root.path().join(task.preserved_path.unwrap())).unwrap(),
        b"local edit"
    );
    assert_eq!(
        fs::read(f.root.path().join("file.txt")).unwrap(),
        b"Drive edit"
    );
}
struct GatedFactory {
    cloud: Arc<FakeProvider>,
    gate: Arc<tokio::sync::Semaphore>,
}
#[async_trait]
impl ProviderFactory for GatedFactory {
    async fn connect(&self, _: &PairConfig) -> Result<Arc<dyn Provider>> {
        let _permit = self.gate.acquire().await.unwrap();
        Ok(self.cloud.clone())
    }
}
#[tokio::test]
async fn another_conflict_instruction_is_accepted_while_the_owner_is_busy() {
    let f = fixture(&["a.txt", "b.txt"]).await;
    let mut client = Database::open(f.profile.path()).unwrap();
    enqueue(&mut client, "a.txt", ConflictChoice::Refresh);
    let gate = Arc::new(tokio::sync::Semaphore::new(0));
    let factory = GatedFactory {
        cloud: f.cloud.clone(),
        gate: gate.clone(),
    };
    let profile = f.profile.path().to_path_buf();
    let cancel = CancellationToken::new();
    let token = cancel.clone();
    let owner = std::thread::spawn(move || {
        let _lock = ProfileLock::acquire(&profile).unwrap();
        let mut db = Database::open(&profile).unwrap();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(engine::run(&mut db, &profile, &factory, token))
            .unwrap();
    });
    let deadline = Instant::now() + Duration::from_secs(8);
    while client.conflict_tasks().unwrap()[0].state != "working" {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let task = enqueue(&mut client, "b.txt", ConflictChoice::Refresh);
    assert_eq!(
        client
            .conflict_tasks()
            .unwrap()
            .iter()
            .find(|j| j.id == task.id)
            .unwrap()
            .state,
        "queued"
    );
    assert!(!client.controls().unwrap().paused);
    gate.add_permits(10);
    while !client
        .conflict_tasks()
        .unwrap()
        .iter()
        .all(|j| j.state == "done")
    {
        assert!(Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    cancel.cancel();
    owner.join().unwrap();
}
#[test]
fn text_detection_covers_utf8_chunk_boundaries_utf16_and_binary_data() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("text");
    let mut text = vec![b'a'; 65535];
    text.extend_from_slice("€\n".as_bytes());
    fs::write(&path, text).unwrap();
    assert!(conflicts::is_text_file(&path).unwrap());
    fs::write(&path, [0xff, 0xfe, b'H', 0, b'i', 0, b'\n', 0]).unwrap();
    assert!(conflicts::is_text_file(&path).unwrap());
    fs::write(&path, b"binary\0data").unwrap();
    assert!(!conflicts::is_text_file(&path).unwrap());
}
