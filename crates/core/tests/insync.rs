use freesync_core::{insync, local};
use rusqlite::Connection;

#[test]
fn imports_only_existing_unselected_paths_for_the_same_account_and_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("files");
    let config = temp.path().join("Insync");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir_all(config.join("data")).unwrap();
    std::fs::create_dir(root.join("[private]*")).unwrap();
    std::fs::create_dir(root.join("selected")).unwrap();
    let settings = Connection::open(config.join("settings.db")).unwrap();
    settings.execute_batch("CREATE TABLE accounts(id TEXT,cloud TEXT,email TEXT); INSERT INTO accounts VALUES('account','gd','fixture@example.invalid');").unwrap();
    let data = Connection::open(config.join("data/gd-account.db")).unwrap();
    data.execute_batch("CREATE TABLE nodes(node_id INTEGER PRIMARY KEY,parent_id INTEGER,sync_flags INTEGER);
        CREATE TABLE fs_items(node_id INTEGER,fs_name TEXT); CREATE TABLE cl_items(node_id INTEGER,cl_name TEXT);
        CREATE TABLE sync_choices(node_id INTEGER,chosen_sync_flags INTEGER);
        INSERT INTO nodes VALUES(-1,NULL,7),(1,-1,0),(2,-1,3),(3,-1,0),(4,-1,0);
        INSERT INTO fs_items VALUES(1,'[private]*'),(2,'selected'),(3,'missing');
        INSERT INTO cl_items VALUES(4,'tombstone');").unwrap();
    data.execute(
        "INSERT INTO fs_items VALUES(-1,?1)",
        [root.to_str().unwrap()],
    )
    .unwrap();
    let root = local::canonical_root(&root).unwrap();
    let rules = insync::exclusions(&config, &root, "FIXTURE@example.invalid").unwrap();
    assert_eq!(rules.len(), 1);
    let exclusions = local::Exclusions::new(&rules).unwrap();
    assert!(exclusions.excludes("[private]*/file"));
    assert!(!exclusions.excludes("private-other/file"));
    assert!(!exclusions.excludes("selected"));
    assert!(
        insync::exclusions(&config, &root, "other@example.invalid")
            .unwrap()
            .is_empty()
    );
    let other = temp.path().join("other");
    std::fs::create_dir(&other).unwrap();
    assert!(
        insync::exclusions(&config, &other, "fixture@example.invalid")
            .unwrap()
            .is_empty()
    );
}
