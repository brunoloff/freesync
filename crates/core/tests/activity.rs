use freesync_core::{
    activity::{Entry, Query, Sort},
    db::Database,
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
        respect_gitignore: true,
        enabled: false,
        poll_secs: 1,
        deletion_limit: 10,
        test_only: true,
    }
}
fn entry(path: Option<&str>, bytes: Option<u64>, at: u64) -> Entry {
    let mut e = Entry::new("upload", "completed", "Verified transfer");
    e.path = path.map(str::to_owned);
    e.size_bytes = bytes;
    e.at_ms = at;
    e
}
#[test]
fn schema_one_upgrade_preserves_sync_state_without_inventing_history() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(temp.path()).unwrap();
    db.save_pair(&pair(temp.path())).unwrap();
    let operation = Operation {
        id: "pending-before-upgrade".into(),
        pair_id: "test".into(),
        path: "file.txt".into(),
        action: Action::Upload,
        expected_local: None,
        expected_remote: None,
        state: OperationState::Prepared,
        attempts: 0,
        retry_at: 0,
        upload_session: None,
        reserved_remote_id: None,
        error: None,
    };
    db.save_operation(&operation).unwrap();
    db.set(
        "preferences",
        &serde_json::json!({"zoom":1.25,"notifications":true}),
    )
    .unwrap();
    db.set(
        "remote:test",
        &serde_json::json!({"cursor":"existing-cursor"}),
    )
    .unwrap();
    drop(db);
    let sql = rusqlite::Connection::open(temp.path().join("state.sqlite3")).unwrap();
    sql.execute_batch("DROP TABLE activity; PRAGMA user_version=1;")
        .unwrap();
    drop(sql);
    let db = Database::open(temp.path()).unwrap();
    assert_eq!(db.schema_version().unwrap(), 2);
    assert_eq!(db.pairs().unwrap()[0].id, "test");
    assert_eq!(db.operations("test").unwrap()[0].id, operation.id);
    assert_eq!(
        db.get::<serde_json::Value>("preferences").unwrap().unwrap()["zoom"],
        1.25
    );
    assert_eq!(
        db.get::<serde_json::Value>("remote:test").unwrap().unwrap()["cursor"],
        "existing-cursor"
    );
    assert_eq!(db.activity(&Query::default()).unwrap().total, 0);
}
#[test]
fn filters_treat_search_as_literal_and_distinguish_zero_from_unknown_size() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(temp.path()).unwrap();
    for e in [
        entry(Some("Ünicode/100%_done.txt"), Some(0), 1),
        entry(Some("small.txt"), Some(10), 2),
        entry(Some("large.txt"), Some(1000), 3),
        entry(None, None, 4),
    ] {
        db.record_activity(&e).unwrap();
    }
    let query = |q: Query| db.activity(&q).unwrap();
    assert_eq!(
        query(Query {
            name: "ÜNICODE/100%_".into(),
            ..Default::default()
        })
        .matching,
        1
    );
    assert_eq!(
        query(Query {
            name: "' OR 1=1 --".into(),
            ..Default::default()
        })
        .matching,
        0
    );
    let p = query(Query {
        min_bytes: Some(0),
        max_bytes: Some(10),
        sort: Sort::SizeAsc,
        ..Default::default()
    });
    assert_eq!(
        p.entries
            .iter()
            .map(|e| e.size_bytes.unwrap())
            .collect::<Vec<_>>(),
        vec![0, 10]
    );
    assert_eq!(
        query(Query {
            since_ms: Some(3),
            outcome: Some("completed".into()),
            ..Default::default()
        })
        .matching,
        2
    );
    assert_eq!(
        query(Query {
            action: Some("download".into()),
            ..Default::default()
        })
        .matching,
        0
    );
    assert_eq!(
        query(Query {
            sort: Sort::SizeDesc,
            ..Default::default()
        })
        .entries[0]
            .size_bytes,
        Some(1000)
    );
    assert!(
        db.activity(&Query {
            min_bytes: Some(11),
            max_bytes: Some(10),
            ..Default::default()
        })
        .is_err()
    );
    assert!(
        db.activity(&Query {
            max_bytes: Some(u64::MAX),
            ..Default::default()
        })
        .is_err()
    );
}
#[test]
fn anchored_pagination_does_not_shift_when_new_activity_arrives() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(temp.path()).unwrap();
    for i in 0..125 {
        db.record_activity(&entry(Some(&format!("file-{i:03}")), Some(i), i))
            .unwrap();
    }
    let first = db.activity(&Query::default()).unwrap();
    assert_eq!(first.entries.len(), 50);
    db.record_activity(&entry(Some("new"), Some(1), 126))
        .unwrap();
    let next = db
        .activity(&Query {
            offset: 50,
            until_id: Some(first.latest_id),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(next.matching, 125);
    assert_eq!(next.entries[0].path.as_deref(), Some("file-074"));
    assert!(
        next.entries
            .iter()
            .all(|e| !first.entries.iter().any(|a| a.id == e.id))
    );
    assert_eq!(
        db.activity(&Query::default()).unwrap().entries[0]
            .path
            .as_deref(),
        Some("new")
    );
    assert_eq!(
        db.activity(&Query {
            limit: Some(10000),
            ..Default::default()
        })
        .unwrap()
        .entries
        .len(),
        100
    );
}
#[test]
fn upload_checkpoint_logging_is_durable_deduplicated_and_excludes_session_secrets() {
    let temp = tempfile::tempdir().unwrap();
    let db = Database::open(temp.path()).unwrap();
    db.save_pair(&pair(temp.path())).unwrap();
    let mut op = Operation {
        id: "durable-op".into(),
        pair_id: "test".into(),
        path: "file.txt".into(),
        action: Action::Upload,
        expected_local: Some(LocalEntry {
            path: "file.txt".into(),
            kind: ItemKind::File,
            fingerprint: Some(Fingerprint {
                size: 100,
                md5: "fixture".into(),
            }),
            modified_ns: 0,
            file_identity: None,
        }),
        expected_remote: None,
        state: OperationState::Prepared,
        attempts: 0,
        retry_at: 0,
        upload_session: Some(UploadSession {
            url: "https://private.invalid/SECRET-session".into(),
            uploaded_bytes: 0,
            total_bytes: 100,
            expected_remote: None,
            target_id: None,
        }),
        reserved_remote_id: None,
        error: None,
    };
    db.save_operation(&op).unwrap();
    db.save_operation(&op).unwrap();
    op.state = OperationState::Running;
    op.attempts = 1;
    db.save_operation(&op).unwrap();
    for done in [20, 21, 22, 40, 41] {
        db.transfer_progress(&op, done, 100).unwrap();
    }
    op.state = OperationState::Retry;
    op.retry_at = 99;
    op.error = Some(Error::new(
        ErrorCode::RateLimited,
        "Service requested a retry",
    ));
    db.save_operation(&op).unwrap();
    let page = db.activity(&Query::default()).unwrap();
    assert_eq!(page.total, 5);
    assert_eq!(
        page.entries[0].details.error_code,
        Some(ErrorCode::RateLimited)
    );
    let json = serde_json::to_string(&page).unwrap();
    assert!(!json.contains("SECRET-session"));
    assert!(!json.contains("private.invalid"));
    assert!(json.contains("durable-op"));
    db.flush_activity_log().unwrap();
    db.flush_activity_log().unwrap();
    drop(db);
    let db = Database::open(temp.path()).unwrap();
    assert_eq!(db.activity(&Query::default()).unwrap().total, 5);
    let text = std::fs::read_to_string(temp.path().join("activity.jsonl")).unwrap();
    assert_eq!(text.lines().count(), 5);
    assert!(!text.contains("SECRET"));
}
#[cfg(unix)]
#[test]
fn failed_log_mirror_preserves_history_and_retries_without_following_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), "untouched").unwrap();
    let db = Database::open(temp.path()).unwrap();
    db.record_activity(&entry(Some("file"), Some(4), 1))
        .unwrap();
    let log = temp.path().join("activity.jsonl");
    symlink(outside.path(), &log).unwrap();
    assert!(db.flush_activity_log().is_err());
    assert_eq!(db.activity(&Query::default()).unwrap().total, 1);
    assert_eq!(
        std::fs::read_to_string(outside.path()).unwrap(),
        "untouched"
    );
    std::fs::remove_file(&log).unwrap();
    db.flush_activity_log().unwrap();
    assert_eq!(
        std::fs::metadata(&log).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::write(&log, vec![b' '; 2 * 1024 * 1024 + 1]).unwrap();
    db.record_activity(&entry(Some("second"), None, 2)).unwrap();
    db.flush_activity_log().unwrap();
    assert!(temp.path().join("activity.previous.jsonl").exists());
    assert_eq!(std::fs::read_to_string(&log).unwrap().lines().count(), 1);
}
