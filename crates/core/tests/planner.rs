use freesync_core::{
    db::Database,
    fake::{FakeProvider, Fault},
    local, planner, remote, *,
};
use std::fs;

fn pair(root: &std::path::Path) -> PairConfig {
    PairConfig {
        id: "test".into(),
        account_email: "fixture@example.invalid".into(),
        local_root: root.into(),
        remote_root_id: "root".into(),
        remote_root_name: "test-freesync".into(),
        root_identity: local::scan(root, &[]).unwrap().root_identity,
        excludes: vec![],
        respect_gitignore: true,
        enabled: false,
        poll_secs: 1,
        deletion_limit: 10,
        test_only: true,
    }
}
async fn make_plan(pair: &PairConfig, cloud: &FakeProvider, base: &[Baseline]) -> Plan {
    planner::plan(
        pair,
        &local::scan(&pair.local_root, &pair.excludes).unwrap(),
        &remote::snapshot(cloud, "root", &pair.excludes, base)
            .await
            .unwrap(),
        base,
    )
    .unwrap()
}
#[tokio::test]
async fn matching_content_is_durable_and_repeated_dry_runs_have_no_work() {
    let local = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    fs::write(local.path().join("same"), b"equal").unwrap();
    let cloud = FakeProvider::default();
    cloud.seed("root", "same", b"equal", ItemKind::File);
    let pair = pair(local.path());
    let l = local::scan(local.path(), &[]).unwrap();
    let r = remote::snapshot(&cloud, "root", &[], &[]).await.unwrap();
    let plan = planner::plan(&pair, &l, &r, &[]).unwrap();
    assert!(plan.operations.is_empty());
    assert_eq!(plan.accepted.len(), 1);
    let mut db = Database::open(profile.path()).unwrap();
    db.save_pair(&pair).unwrap();
    db.persist_plan(&pair.id, &plan, &l, &r).unwrap();
    drop(db);
    let db = Database::open(profile.path()).unwrap();
    let base = db.baselines(&pair.id).unwrap();
    assert!(make_plan(&pair, &cloud, &base).await.operations.is_empty());
    fs::write(local.path().join("same"), b"local edit").unwrap();
    cloud.edit(&base[0].remote.id, b"remote edit");
    let changed = make_plan(&pair, &cloud, &base).await;
    assert_eq!(changed.conflicts.len(), 1);
    assert!(changed.operations.is_empty());
}
#[tokio::test]
async fn initial_mismatch_and_duplicate_directory_names_never_overwrite() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("same"), b"local").unwrap();
    let cloud = FakeProvider::default();
    cloud.seed("root", "same", b"remote", ItemKind::File);
    let d1 = cloud.seed("root", "ambiguous", &[], ItemKind::Folder);
    let d2 = cloud.seed("root", "ambiguous", &[], ItemKind::Folder);
    cloud.seed(&d1, "a", b"a", ItemKind::File);
    cloud.seed(&d2, "b", b"b", ItemKind::File);
    let plan = make_plan(&pair(root.path()), &cloud, &[]).await;
    assert_eq!(plan.conflicts.len(), 2);
    assert!(plan.operations.is_empty());
}
#[tokio::test]
async fn moves_are_recognized_and_overlapping_edits_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("old")).unwrap();
    fs::write(root.path().join("old/file"), b"value").unwrap();
    let cloud = FakeProvider::default();
    let folder = cloud.seed("root", "old", &[], ItemKind::Folder);
    cloud.seed(&folder, "file", b"value", ItemKind::File);
    let pair = pair(root.path());
    let base = make_plan(&pair, &cloud, &[]).await.accepted;
    fs::rename(root.path().join("old"), root.path().join("new")).unwrap();
    let plan = make_plan(&pair, &cloud, &base).await;
    assert_eq!(plan.operations.len(), 1);
    assert!(matches!(&plan.operations[0].action,Action::MoveRemote{from} if from=="old"));
    fs::write(root.path().join("new/file"), b"edited at same time").unwrap();
    let conflict = make_plan(&pair, &cloud, &base).await;
    assert!(!conflict.conflicts.is_empty());
    assert!(conflict.operations.is_empty());
}
#[tokio::test]
async fn unknown_remote_removal_and_replaced_local_root_cannot_infer_deletion() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("same"), b"value").unwrap();
    let cloud = FakeProvider::default();
    let id = cloud.seed("root", "same", b"value", ItemKind::File);
    let pair = pair(root.path());
    let base = make_plan(&pair, &cloud, &[]).await.accepted;
    cloud.deny(&id);
    // Simulate a change-feed removal / lost access. It is not proof of trash.
    let r = RemoteInventory {
        cursor: Some("cursor".into()),
        ..Default::default()
    };
    let l = local::scan(root.path(), &[]).unwrap();
    let p = planner::plan(&pair, &l, &r, &base).unwrap();
    assert!(p.operations.is_empty());
    assert_eq!(p.conflicts.len(), 1);
    let mut replaced = l.clone();
    replaced.root_identity = "other-disk".into();
    assert!(planner::plan(&pair, &replaced, &r, &base).is_err());
    cloud.inject("children", Fault::Permission);
    assert!(remote::snapshot(&cloud, "root", &[], &base).await.is_err());
}
#[tokio::test]
async fn folder_deletion_stops_if_remote_children_changed_or_were_added() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("folder")).unwrap();
    fs::write(root.path().join("folder/file"), b"value").unwrap();
    let cloud = FakeProvider::default();
    let d = cloud.seed("root", "folder", &[], ItemKind::Folder);
    let f = cloud.seed(&d, "file", b"value", ItemKind::File);
    let pair = pair(root.path());
    let base = make_plan(&pair, &cloud, &[]).await.accepted;
    fs::remove_dir_all(root.path().join("folder")).unwrap();
    cloud.edit(&f, b"new remote edit");
    cloud.seed(&d, "new", b"new remote file", ItemKind::File);
    let plan = make_plan(&pair, &cloud, &base).await;
    assert!(
        !plan
            .operations
            .iter()
            .any(|o| o.path == "folder" && matches!(o.action, Action::TrashRemote))
    );
    assert!(!plan.conflicts.is_empty());
}
#[tokio::test]
async fn excluded_or_skipped_baselines_are_not_deletions_and_plan_survives_restart() {
    let root = tempfile::tempdir().unwrap();
    let profile = tempfile::tempdir().unwrap();
    fs::write(root.path().join("pending"), b"new").unwrap();
    let cloud = FakeProvider::default();
    let pair = pair(root.path());
    let plan = make_plan(&pair, &cloud, &[]).await;
    assert_eq!(plan.operations.len(), 1);
    let mut db = Database::open(profile.path()).unwrap();
    db.save_pair(&pair).unwrap();
    db.persist_plan(
        &pair.id,
        &plan,
        &local::scan(root.path(), &[]).unwrap(),
        &remote::snapshot(&cloud, "root", &[], &[]).await.unwrap(),
    )
    .unwrap();
    drop(db);
    let reopened = Database::open(profile.path()).unwrap();
    assert_eq!(
        reopened.operations(&pair.id).unwrap()[0].id,
        plan.operations[0].id
    );
    let output = planner::preview(&plan).to_string();
    assert!(!output.contains("upload_session"));
    cloud.seed("root", "pending", b"new", ItemKind::File);
    let base = make_plan(&pair, &cloud, &[]).await.accepted;
    let mut excluded = pair;
    excluded.excludes = vec!["pending".into()];
    assert!(
        make_plan(&excluded, &cloud, &base)
            .await
            .operations
            .is_empty()
    );
}
