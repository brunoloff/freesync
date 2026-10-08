use freesync_core::{ErrorCode, local, watcher::LocalWatcher};
use std::{fs, time::Duration};
use tokio_util::sync::CancellationToken;

#[test]
fn scans_converge_after_create_edit_atomic_save_move_and_delete() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join(".hidden"), b"included").unwrap();
    fs::write(root.join("a"), b"first").unwrap();
    let first = local::scan(root, &[]).unwrap();
    assert_eq!(first.entries.len(), 2);
    fs::write(root.join("a"), b"second").unwrap();
    let edited = local::scan(root, &[]).unwrap();
    assert_ne!(
        first.entries["a"].fingerprint,
        edited.entries["a"].fingerprint
    );
    fs::write(root.join("save.tmp"), b"atomic").unwrap();
    fs::rename(root.join("save.tmp"), root.join("a")).unwrap();
    fs::create_dir_all(root.join("nested/deep")).unwrap();
    fs::rename(root.join("a"), root.join("nested/deep/renamed")).unwrap();
    let moved = local::scan(root, &[]).unwrap();
    assert!(moved.entries.contains_key("nested/deep/renamed"));
    assert!(!moved.entries.contains_key("a"));
    assert_eq!(moved.entries, local::scan(root, &[]).unwrap().entries);
    fs::remove_file(root.join("nested/deep/renamed")).unwrap();
    assert!(
        !local::scan(root, &[])
            .unwrap()
            .entries
            .contains_key("nested/deep/renamed")
    );
}
#[test]
fn exclusions_are_recursive_and_root_loss_is_an_error_not_empty_scan() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir_all(root.join("skip/nested")).unwrap();
    fs::write(root.join("skip/nested/file"), b"ignored").unwrap();
    fs::write(root.join("keep"), b"kept").unwrap();
    let result = local::scan(&root, &["skip".into()]).unwrap();
    assert_eq!(result.entries.len(), 1);
    fs::rename(&root, temp.path().join("unavailable")).unwrap();
    assert!(local::scan(&root, &[]).is_err());
    fs::create_dir(&root).unwrap();
    assert_ne!(
        result.root_identity,
        local::scan(&root, &[]).unwrap().root_identity
    );
}
#[test]
fn paths_and_overlapping_roots_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    for name in [
        "../escape",
        "a/../escape",
        "/absolute",
        "C:/escape",
        "a\\b",
        "a//b",
        ".",
    ] {
        assert!(local::safe_join(temp.path(), name).is_err());
    }
    let nested = temp.path().join("nested");
    fs::create_dir(&nested).unwrap();
    assert!(local::check_nonoverlap(&nested, &[temp.path().into()], temp.path()).is_err());
}
#[cfg(unix)]
#[test]
fn links_do_not_escape_and_unreadable_roots_fail_closed() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir(&root).unwrap();
    symlink(temp.path(), root.join("escape")).unwrap();
    assert!(local::scan(&root, &[]).unwrap().entries.is_empty());
    assert_eq!(
        local::safe_join(&root, "escape/file").err().unwrap().code,
        ErrorCode::UnsafePath
    );
    fs::set_permissions(&root, fs::Permissions::from_mode(0o000)).unwrap();
    let result = local::scan(&root, &[]);
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(result.err().unwrap().code, ErrorCode::Permission);
}
#[tokio::test]
async fn polling_recovers_missed_events_root_reappearance_and_cancels() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir(&root).unwrap();
    let mut watcher = LocalWatcher::new(&root, &[], Duration::from_millis(20), true).unwrap();
    let cancel = CancellationToken::new();
    fs::write(root.join("missed"), b"no native event").unwrap();
    assert!(watcher.wait(&cancel).await);
    assert!(
        local::scan(&root, &[])
            .unwrap()
            .entries
            .contains_key("missed")
    );
    fs::rename(&root, temp.path().join("away")).unwrap();
    assert!(watcher.wait(&cancel).await);
    assert!(local::scan(&root, &[]).is_err());
    fs::rename(temp.path().join("away"), &root).unwrap();
    assert!(watcher.wait(&cancel).await);
    assert!(
        local::scan(&root, &[])
            .unwrap()
            .entries
            .contains_key("missed")
    );
    cancel.cancel();
    assert!(!watcher.wait(&cancel).await);
}
#[tokio::test]
async fn native_watcher_notices_new_nested_directories_and_atomic_editor_saves() {
    let temp = tempfile::tempdir().unwrap();
    // A native event must arrive well before the scan fallback; otherwise this
    // test would also pass with no working recursive notification backend.
    let mut watcher = LocalWatcher::new(temp.path(), &[], Duration::from_secs(30), false).unwrap();
    assert_eq!(watcher.backend(), "native_with_rescan");
    fs::create_dir_all(temp.path().join("new/deep")).unwrap();
    fs::write(temp.path().join("new/deep/editor.tmp"), b"saved").unwrap();
    fs::rename(
        temp.path().join("new/deep/editor.tmp"),
        temp.path().join("new/deep/file"),
    )
    .unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(3),
            watcher.wait(&CancellationToken::new())
        )
        .await
        .unwrap()
    );
    assert!(
        local::scan(temp.path(), &[])
            .unwrap()
            .entries
            .contains_key("new/deep/file")
    );
}

#[tokio::test]
async fn inventory_reads_and_sibling_changes_do_not_trigger_reconciliation_but_edits_do() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("root");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("file"), b"initial").unwrap();
    let mut watcher = LocalWatcher::new(&root, &[], Duration::from_secs(30), false).unwrap();
    assert_eq!(watcher.backend(), "native_with_rescan");
    let cancel = CancellationToken::new();
    local::scan(&root, &[]).unwrap();
    fs::write(
        temp.path().join("unrelated-sibling"),
        b"outside the selected root",
    )
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(800), watcher.wait(&cancel))
            .await
            .is_err(),
        "Reading the inventory or changing a sibling must not request reconciliation"
    );
    fs::write(root.join("file"), b"edited").unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), watcher.wait(&cancel))
            .await
            .unwrap()
    );
    assert_eq!(
        local::scan(&root, &[]).unwrap().entries["file"]
            .fingerprint
            .as_ref()
            .unwrap()
            .size,
        6
    );
}
