use freesync_core::{
    fake::{FakeProvider, Fault},
    remote::{Ancestry, ancestry, snapshot},
    *,
};

#[tokio::test]
async fn paginated_inventory_retains_duplicates_and_applies_exclusions() {
    let cloud = FakeProvider::new(1);
    let folder = cloud.seed("root", "folder", &[], ItemKind::Folder);
    cloud.seed(&folder, "a", b"a", ItemKind::File);
    cloud.seed(&folder, "a", b"duplicate", ItemKind::File);
    cloud.seed("root", "excluded", b"hidden", ItemKind::File);
    let inventory = snapshot(&cloud, "root", &["excluded".into()], &[])
        .await
        .unwrap();
    assert_eq!(inventory.entries["folder/a"].len(), 2);
    assert!(!inventory.entries.contains_key("excluded"));
    assert!(inventory.cursor.is_some());
    assert_eq!(
        ancestry(&cloud, &folder, "root").await.unwrap(),
        Ancestry::Inside
    );
}
#[tokio::test]
async fn listing_failure_is_not_an_empty_remote_inventory() {
    let cloud = FakeProvider::default();
    cloud.inject("children", Fault::Permission);
    assert_eq!(
        snapshot(&cloud, "root", &[], &[]).await.err().unwrap().code,
        ErrorCode::Permission
    );
}
#[tokio::test]
async fn unsafe_remote_names_fail_before_any_local_path_mapping() {
    let cloud = FakeProvider::default();
    cloud.seed("root", "../outside", b"no", ItemKind::File);
    assert!(snapshot(&cloud, "root", &[], &[]).await.is_err());
}

#[tokio::test]
async fn changes_during_pagination_trigger_a_fresh_consistent_traversal() {
    let cloud = FakeProvider::new(1);
    let id = cloud.seed("root", "a", b"before", ItemKind::File);
    cloud.seed("root", "b", b"second page", ItemKind::File);
    let before = cloud.get(&id).await.unwrap();
    cloud.change_after_listing_calls(2, &id, b"during pagination".to_vec());
    let inventory = snapshot(&cloud, "root", &[], &[]).await.unwrap();
    assert_ne!(inventory.entries["a"][0].fingerprint, before.fingerprint);
    assert_eq!(inventory.entries["a"][0], cloud.get(&id).await.unwrap());
}
