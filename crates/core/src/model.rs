use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub email: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    File,
    Folder,
    NativeDocument,
    Shortcut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fingerprint {
    pub size: u64,
    pub md5: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalEntry {
    pub path: String,
    pub kind: ItemKind,
    pub fingerprint: Option<Fingerprint>,
    pub modified_ns: u128,
    pub file_identity: Option<String>,
}
impl LocalEntry {
    pub fn content_eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.fingerprint == other.fingerprint
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteItem {
    pub id: String,
    pub name: String,
    pub parents: Vec<String>,
    pub kind: ItemKind,
    pub fingerprint: Option<Fingerprint>,
    pub version: String,
    pub modified_time: Option<String>,
    pub etag: Option<String>,
    pub trashed: bool,
    pub can_download: bool,
    pub can_edit: bool,
    pub can_add_children: bool,
    pub operation_id: Option<String>,
}
impl RemoteItem {
    pub fn content_eq(&self, other: &Self) -> bool {
        self.kind == other.kind && self.fingerprint == other.fingerprint
    }
    pub fn matches_local(&self, local: &LocalEntry) -> bool {
        self.kind == local.kind
            && (self.kind == ItemKind::Folder
                || self.fingerprint.is_some() && self.fingerprint == local.fingerprint)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedItem {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalInventory {
    pub root_identity: String,
    pub entries: BTreeMap<String, LocalEntry>,
    pub skipped: Vec<SkippedItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RemoteInventory {
    /// Multiple entries are retained, because Drive permits duplicate names.
    pub entries: BTreeMap<String, Vec<RemoteItem>>,
    pub cursor: Option<String>,
    pub confirmed_removed: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairConfig {
    pub id: String,
    pub account_email: String,
    pub local_root: PathBuf,
    pub remote_root_id: String,
    pub remote_root_name: String,
    pub root_identity: String,
    pub excludes: Vec<String>,
    pub enabled: bool,
    pub poll_secs: u64,
    pub deletion_limit: usize,
    /// Autonomous development is restricted to explicitly validated test roots.
    pub test_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    pub path: String,
    pub local: LocalEntry,
    pub remote: RemoteItem,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    Upload,
    Download,
    CreateRemoteFolder,
    CreateLocalFolder,
    MoveRemote { from: String },
    MoveLocal { from: String },
    TrashRemote,
    RecycleLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Prepared,
    Running,
    Retry,
    Done,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadSession {
    pub url: String,
    pub uploaded_bytes: u64,
    pub total_bytes: u64,
    #[serde(default)]
    pub expected_remote: Option<RemoteItem>,
    #[serde(default)]
    pub target_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: String,
    pub pair_id: String,
    pub path: String,
    pub action: Action,
    pub expected_local: Option<LocalEntry>,
    pub expected_remote: Option<RemoteItem>,
    pub state: OperationState,
    pub attempts: u32,
    pub retry_at: u64,
    pub upload_session: Option<UploadSession>,
    #[serde(default)]
    pub reserved_remote_id: Option<String>,
    pub error: Option<crate::Error>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub id: String,
    pub pair_id: String,
    pub path: String,
    pub reason: String,
    pub local: Option<LocalEntry>,
    pub remote: Option<RemoteItem>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Plan {
    pub operations: Vec<Operation>,
    pub conflicts: Vec<Conflict>,
    pub skipped: Vec<SkippedItem>,
    pub accepted: Vec<Baseline>,
    pub forgotten: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PairStatus {
    pub pair_id: String,
    pub state: String,
    pub queued: usize,
    pub conflicts: usize,
    pub last_sync: Option<u64>,
    pub error: Option<crate::Error>,
    #[serde(default)]
    pub current_path: Option<String>,
    #[serde(default)]
    pub progress_bytes: u64,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub retry_at: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Controls {
    pub paused: bool,
    pub sync_now: u64,
    pub quit: bool,
}
