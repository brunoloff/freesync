use freesync_core::{
    db::Database,
    fake::{FakeProvider, Fault},
    profile::ProfileLock,
    provider::{UploadProgress, UploadRequest},
    *,
};

fn pair(root: &std::path::Path) -> PairConfig {
    PairConfig {
        id: "test".into(),
        account_email: "fixture@example.invalid".into(),
        local_root: root.into(),
        remote_root_id: "root".into(),
        remote_root_name: "test".into(),
        root_identity: "fixture".into(),
        excludes: vec![],
        enabled: false,
        poll_secs: 1,
        deletion_limit: 10,
        test_only: true,
    }
}
#[test]
fn profile_owner_is_exclusive_and_releases_on_drop() {
    let temp = tempfile::tempdir().unwrap();
    let first = ProfileLock::acquire(temp.path()).unwrap();
    assert_eq!(
        ProfileLock::acquire(temp.path()).err().unwrap().code,
        ErrorCode::LockBusy
    );
    drop(first);
    assert!(ProfileLock::acquire(temp.path()).is_ok());
}
#[test]
fn migrations_persist_and_profiles_are_isolated() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let db = Database::open(a.path()).unwrap();
    assert_eq!(db.schema_version().unwrap(), 1);
    db.save_pair(&pair(a.path())).unwrap();
    drop(db);
    assert_eq!(Database::open(a.path()).unwrap().pairs().unwrap().len(), 1);
    assert!(
        Database::open(b.path())
            .unwrap()
            .pairs()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn failed_plan_transaction_does_not_advance_inventory_or_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let mut db = Database::open(temp.path()).unwrap();
    let local = LocalInventory {
        root_identity: "id".into(),
        entries: Default::default(),
        skipped: vec![],
    };
    let remote = RemoteInventory {
        cursor: Some("new-cursor".into()),
        ..Default::default()
    };
    let plan = Plan {
        operations: vec![Operation {
            id: "work".into(),
            pair_id: "missing-pair".into(),
            path: "file".into(),
            action: Action::Upload,
            expected_local: None,
            expected_remote: None,
            state: OperationState::Prepared,
            attempts: 0,
            retry_at: 0,
            upload_session: None,
            reserved_remote_id: None,
            error: None,
        }],
        ..Default::default()
    };
    assert!(
        db.persist_plan("missing-pair", &plan, &local, &remote)
            .is_err()
    );
    assert!(
        db.get::<RemoteInventory>("remote:missing-pair")
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn fake_provider_pages_changes_and_recovers_uncertain_upload() {
    let cloud = FakeProvider::new(1);
    let cursor = cloud.start_cursor().await.unwrap();
    cloud.seed("root", "a", b"a", ItemKind::File);
    cloud.seed("root", "b", b"b", ItemKind::File);
    let page = cloud.children("root", None).await.unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(
        cloud
            .children("root", page.next.as_deref())
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let changes = cloud.changes(&cursor).await.unwrap();
    assert!(changes.next.is_some());
    let end = cloud.changes(changes.next.as_ref().unwrap()).await.unwrap();
    assert!(end.new_cursor.is_some());
    let request = UploadRequest {
        operation_id: "durable-op".into(),
        parent_id: "root".into(),
        name: "new".into(),
        existing: None,
        size: 6,
        reserved_id: Some(cloud.reserve_id().await.unwrap()),
    };
    let mut session = cloud.start_upload(&request).await.unwrap();
    match cloud.upload_chunk(&session, b"abc".to_vec()).await.unwrap() {
        UploadProgress::Continue(n) => session.uploaded_bytes = n,
        _ => panic!(),
    }
    cloud.inject("upload_chunk", Fault::UncertainSuccess);
    assert_eq!(
        cloud
            .upload_chunk(&session, b"def".to_vec())
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::AmbiguousOutcome
    );
    let done = match cloud.upload_status(&session).await.unwrap() {
        UploadProgress::Complete(i) => i,
        _ => panic!(),
    };
    assert_eq!(cloud.download(&done.id, 0, 1024).await.unwrap(), b"abcdef");
    assert_eq!(
        cloud
            .find_operation("root", "durable-op")
            .await
            .unwrap()
            .len(),
        1
    );
}
#[tokio::test]
async fn fake_faults_distinguish_permissions_transience_rate_limits_and_stale_versions() {
    let cloud = FakeProvider::default();
    for (fault, code) in [
        (Fault::Permission, ErrorCode::Permission),
        (Fault::Timeout, ErrorCode::Transient),
        (Fault::RateLimit, ErrorCode::RateLimited),
    ] {
        cloud.inject("identity", fault);
        assert_eq!(cloud.identity().await.err().unwrap().code, code);
    }
    let id = cloud.seed("root", "file", b"before", ItemKind::File);
    let old = cloud.get(&id).await.unwrap();
    cloud.edit(&id, b"after");
    assert_eq!(
        cloud.trash(&old).await.err().unwrap().code,
        ErrorCode::Conflict
    );
    cloud.change_during_download(&id, b"during".to_vec());
    let snapshot = cloud.get(&id).await.unwrap();
    cloud.download(&id, 0, 1024).await.unwrap();
    assert_ne!(cloud.get(&id).await.unwrap().version, snapshot.version);
}
