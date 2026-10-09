//! Private, durable activity history. Only explicit diagnostic fields are stored;
//! operation/session objects and file contents never enter this log.
use crate::{Action, Conflict, ConflictTask, ErrorCode, Operation, OperationState, Result};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Details {
    pub operation_id: Option<String>,
    pub from_path: Option<String>,
    pub recovery_path: Option<String>,
    pub attempt: Option<u32>,
    pub local_bytes: Option<u64>,
    pub drive_bytes: Option<u64>,
    pub bytes_done: Option<u64>,
    pub retry_at: Option<u64>,
    pub error_code: Option<ErrorCode>,
    pub duration_ms: Option<u64>,
    pub changes: Option<usize>,
    pub conflicts: Option<usize>,
    pub skipped: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: i64,
    pub at_ms: u64,
    pub action: String,
    pub outcome: String,
    pub pair_id: Option<String>,
    pub path: Option<String>,
    pub size_bytes: Option<u64>,
    pub message: String,
    pub details: Details,
}
impl Entry {
    pub fn new(action: &str, outcome: &str, message: impl Into<String>) -> Self {
        Self {
            id: 0,
            at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            action: action.into(),
            outcome: outcome.into(),
            pair_id: None,
            path: None,
            size_bytes: None,
            message: message.into(),
            details: Details::default(),
        }
    }
    pub fn operation(op: &Operation, outcome: &str) -> Self {
        let action = match op.action {
            Action::Upload => "upload",
            Action::Download => "download",
            Action::CreateRemoteFolder => "create_drive_folder",
            Action::CreateLocalFolder => "create_local_folder",
            Action::MoveRemote { .. } => "move_drive",
            Action::MoveLocal { .. } => "move_local",
            Action::TrashRemote => "drive_trash",
            Action::RecycleLocal => "local_recovery",
        };
        let mut e = Self::new(
            action,
            outcome,
            op.error.as_ref().map_or_else(
                || format!("{}: {outcome}", action.replace('_', " ")),
                |error| error.message.clone(),
            ),
        );
        e.pair_id = Some(op.pair_id.clone());
        e.path = Some(op.path.clone());
        e.details.operation_id = Some(op.id.clone());
        e.details.attempt = Some(op.attempts);
        e.details.local_bytes = op
            .expected_local
            .as_ref()
            .and_then(|i| i.fingerprint.as_ref())
            .map(|f| f.size);
        e.details.drive_bytes = op
            .expected_remote
            .as_ref()
            .and_then(|i| i.fingerprint.as_ref())
            .map(|f| f.size);
        e.size_bytes = if matches!(op.action, Action::Upload) {
            e.details.local_bytes
        } else {
            e.details.drive_bytes.or(e.details.local_bytes)
        };
        e.details.from_path = match &op.action {
            Action::MoveLocal { from } | Action::MoveRemote { from } => Some(from.clone()),
            _ => None,
        };
        e.details.retry_at = (op.retry_at > 0).then_some(op.retry_at);
        e.details.error_code = op.error.as_ref().map(|e| e.code);
        e
    }
    pub fn conflict(c: &Conflict) -> Self {
        let mut e = Self::new("conflict", "conflict", c.reason.clone());
        e.pair_id = Some(c.pair_id.clone());
        e.path = Some(c.path.clone());
        e.details.local_bytes = c
            .local
            .as_ref()
            .and_then(|i| i.fingerprint.as_ref())
            .map(|f| f.size);
        e.details.drive_bytes = c
            .remote
            .as_ref()
            .and_then(|i| i.fingerprint.as_ref())
            .map(|f| f.size);
        e.size_bytes = e
            .details
            .local_bytes
            .into_iter()
            .chain(e.details.drive_bytes)
            .max();
        e
    }
    pub fn decision(t: &ConflictTask) -> Self {
        let choice = match t.choice {
            crate::ConflictChoice::KeepBoth => "keep_both",
            crate::ConflictChoice::UseLocal => "use_local",
            crate::ConflictChoice::UseDrive => "use_drive",
            crate::ConflictChoice::Compare => "compare",
            crate::ConflictChoice::Refresh => "refresh_comparison",
        };
        let mut e = Self::conflict(&t.conflict);
        e.action = choice.into();
        e.outcome = match t.state.as_str() {
            "done" => "completed",
            "failed" => "error",
            "queued" => "queued",
            _ => "started",
        }
        .into();
        e.message = t.error.as_ref().map_or_else(
            || {
                format!(
                    "{}: {}",
                    choice.replace('_', " "),
                    t.state.replace('_', " ")
                )
            },
            |error| error.message.clone(),
        );
        e.details.operation_id = Some(t.id.clone());
        e.details.error_code = t.error.as_ref().map(|e| e.code);
        if let Some(path) = &t.preserved_path {
            e.details.recovery_path = Some(path.clone());
        }
        e
    }
}
fn integer(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| {
        crate::Error::new(
            ErrorCode::InvalidConfig,
            "Size or date exceeds the supported range.",
        )
    })
}
pub(crate) fn insert(db: &Connection, entry: &Entry, key: Option<&str>) -> Result<()> {
    db.execute("INSERT OR IGNORE INTO activity(at_ms,action,outcome,pair,path,search_path,size_bytes,event_key,json) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
        params![integer(entry.at_ms)?,entry.action,entry.outcome,entry.pair_id,entry.path,entry.path.as_ref().map(|p|p.to_lowercase()),entry.size_bytes.map(integer).transpose()?,key,serde_json::to_string(entry)?])?;
    Ok(())
}
pub(crate) fn operation(db: &Connection, op: &Operation) -> Result<()> {
    let outcome = match op.state {
        OperationState::Prepared => "queued",
        OperationState::Running => "started",
        OperationState::Retry => "retry",
        OperationState::Done => "completed",
        OperationState::Conflict => "conflict",
    };
    insert(
        db,
        &Entry::operation(op, outcome),
        Some(&format!("operation:{}:{outcome}:{}", op.id, op.attempts)),
    )
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    NameAsc,
    NameDesc,
    SizeAsc,
    SizeDesc,
    ActionAsc,
    OutcomeAsc,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Query {
    pub name: String,
    pub min_bytes: Option<u64>,
    pub max_bytes: Option<u64>,
    pub action: Option<String>,
    pub outcome: Option<String>,
    pub since_ms: Option<u64>,
    pub sort: Sort,
    pub offset: u32,
    pub limit: Option<u32>,
    pub until_id: Option<i64>,
}
#[derive(Debug, Serialize)]
pub struct Page {
    pub entries: Vec<Entry>,
    pub matching: u64,
    pub total: u64,
    pub latest_id: i64,
    pub limit: u32,
}
pub(crate) fn query(db: &Connection, q: &Query) -> Result<Page> {
    if q.min_bytes.zip(q.max_bytes).is_some_and(|(a, b)| a > b) {
        return Err(crate::Error::new(
            ErrorCode::InvalidConfig,
            "Minimum size must not exceed maximum size.",
        ));
    }
    let latest_id: i64 =
        db.query_row("SELECT coalesce(max(id),0) FROM activity", [], |r| r.get(0))?;
    let anchor = q.until_id.unwrap_or(latest_id).min(latest_id);
    let filter = "id<=?1 AND (?2='' OR instr(search_path,?2)>0) AND (?3 IS NULL OR size_bytes>=?3) AND (?4 IS NULL OR size_bytes<=?4) AND (?5 IS NULL OR action=?5) AND (?6 IS NULL OR outcome=?6) AND (?7 IS NULL OR at_ms>=?7)";
    let name = q.name.to_lowercase();
    let min_bytes = q.min_bytes.map(integer).transpose()?;
    let max_bytes = q.max_bytes.map(integer).transpose()?;
    let since_ms = q.since_ms.map(integer).transpose()?;
    let params = params![
        anchor, name, min_bytes, max_bytes, q.action, q.outcome, since_ms
    ];
    let matching: i64 = db.query_row(
        &format!("SELECT count(*) FROM activity WHERE {filter}"),
        params,
        |r| r.get(0),
    )?;
    let total: i64 = db.query_row("SELECT count(*) FROM activity", [], |r| r.get(0))?;
    let order = match q.sort {
        Sort::Newest => "at_ms DESC,id DESC",
        Sort::Oldest => "at_ms ASC,id ASC",
        Sort::NameAsc => "path IS NULL,path COLLATE NOCASE ASC,id DESC",
        Sort::NameDesc => "path IS NULL,path COLLATE NOCASE DESC,id DESC",
        Sort::SizeAsc => "size_bytes IS NULL,size_bytes ASC,id DESC",
        Sort::SizeDesc => "size_bytes IS NULL,size_bytes DESC,id DESC",
        Sort::ActionAsc => "action ASC,id DESC",
        Sort::OutcomeAsc => "outcome ASC,id DESC",
    };
    let limit = q.limit.unwrap_or(50).clamp(1, 100);
    let mut statement = db.prepare(&format!(
        "SELECT id,json FROM activity WHERE {filter} ORDER BY {order} LIMIT ?8 OFFSET ?9"
    ))?;
    let rows = statement.query_map(
        params![
            anchor, name, min_bytes, max_bytes, q.action, q.outcome, since_ms, limit, q.offset
        ],
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    )?;
    let mut entries = vec![];
    for row in rows {
        let (id, json) = row?;
        let mut e: Entry = serde_json::from_str(&json)?;
        e.id = id;
        entries.push(e);
    }
    Ok(Page {
        entries,
        matching: matching as u64,
        total: total as u64,
        latest_id: anchor,
        limit,
    })
}
