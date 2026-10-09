//! Read-only, checkpointed adoption. A report is never transfer authority.
use crate::{local, profile, *};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Source {
    pub account_email: String,
    pub local_root: PathBuf,
    pub remote_root_id: String,
    pub root_identity: String,
    pub excludes: Vec<String>,
    pub exclusion_source: String,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Progress {
    pub phase: String,
    pub remote_items: u64,
    pub local_items: u64,
    pub hashed_bytes: u64,
    pub reused_hashes: u64,
    pub current_path: Option<String>,
    pub revision: u64,
    #[serde(default)]
    pub drive_passes: u64,
    pub completed_at: Option<u64>,
    pub error: Option<Error>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    pub path: String,
    pub status: String,
    pub reason: String,
    pub size_bytes: Option<u64>,
    pub remote_id: Option<String>,
    #[serde(default)]
    pub kind: Option<ItemKind>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub source: Source,
    pub progress: Progress,
    pub counts: BTreeMap<String, u64>,
    pub findings: Vec<Finding>,
    pub matching: u64,
    pub offset: u64,
    pub directory: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Authorization {
    pub pair: PairConfig,
    #[serde(default)]
    pub scope: String,
    pub remote_root: RemoteItem,
    pub manifest_revision: u64,
    pub approved_at: u64,
}
impl Authorization {
    pub fn verify(&self, pair: &PairConfig) -> Result<()> {
        let p = &self.pair;
        if self.remote_root.id != pair.remote_root_id
            || self.remote_root.kind != ItemKind::Folder
            || self.remote_root.trashed
            || pair.test_only
            || pair.id != p.id
            || pair.account_email != p.account_email
            || pair.local_root != p.local_root
            || pair.root_identity != p.root_identity
            || pair.remote_root_id != p.remote_root_id
            || pair.excludes != p.excludes
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "The reviewed adoption scope changed. Review its roots and exclusions again.",
            ));
        }
        let root = local::canonical_root(&pair.local_root)?;
        if root != pair.local_root
            || local::identity(&fs::metadata(&root)?).as_deref()
                != Some(pair.root_identity.as_str())
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "The reviewed adoption root was replaced. Transfers are stopped.",
            ));
        }
        Ok(())
    }
}
pub fn authorize(db: &crate::db::Database, pair: &PairConfig) -> Result<Authorization> {
    let grant: Authorization = db
        .get(&format!("adoption_authorization:{}", pair.id))?
        .ok_or_else(|| {
            Error::new(
                ErrorCode::Permission,
                "Review and activate an adoption scope before syncing this folder.",
            )
        })?;
    grant.verify(pair)?;
    Ok(grant)
}
pub fn literal_glob(path: &str) -> String {
    path.chars()
        .flat_map(|c| {
            if matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '\\') {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}
/// Database stays outside the source tree and contains metadata, never file contents.
pub struct Manifest {
    db: Connection,
    pub directory: PathBuf,
}
fn cancelled(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Error::new(
            ErrorCode::Cancelled,
            "Inventory cancelled. Resume rechecks saved comparisons.",
        ))
    } else {
        Ok(())
    }
}
impl Manifest {
    pub fn open(directory: &Path) -> Result<Self> {
        profile::private_directory(directory)?;
        let db = Connection::open(directory.join("manifest.sqlite3"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
            CREATE TABLE IF NOT EXISTS state(key TEXT PRIMARY KEY,json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS remote(id TEXT PRIMARY KEY,json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS unavailable(id TEXT PRIMARY KEY,json TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS edges(parent TEXT NOT NULL,id TEXT NOT NULL,PRIMARY KEY(parent,id));
            CREATE INDEX IF NOT EXISTS edge_item ON edges(id);
            CREATE TABLE IF NOT EXISTS paths(path TEXT NOT NULL,id TEXT NOT NULL,PRIMARY KEY(path,id));
            CREATE INDEX IF NOT EXISTS path_item ON paths(id);
            CREATE TABLE IF NOT EXISTS local(path TEXT PRIMARY KEY,json TEXT NOT NULL,seen INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS issues(path TEXT PRIMARY KEY,reason TEXT NOT NULL,side TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS findings(path TEXT PRIMARY KEY,status TEXT NOT NULL,json TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS finding_status ON findings(status,path);")?;
        Ok(Self {
            db,
            directory: directory.canonicalize()?,
        })
    }
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let s: Option<String> = self
            .db
            .query_row("SELECT json FROM state WHERE key=?1", [key], |r| r.get(0))
            .optional()?;
        s.map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }
    pub fn set<T: Serialize>(&self, key: &str, value: &T) -> Result<()> {
        self.db.execute(
            "INSERT INTO state VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET json=excluded.json",
            params![key, serde_json::to_string(value)?],
        )?;
        Ok(())
    }
    pub fn source(&self) -> Result<Source> {
        self.get("source")?.ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidConfig,
                "Start a read-only adoption report first.",
            )
        })
    }
    pub fn observed_remote(&self, id: &str) -> Result<Option<RemoteItem>> {
        let json: Option<String> = self
            .db
            .query_row("SELECT json FROM remote WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        json.map(|json| serde_json::from_str(&json).map_err(Into::into))
            .transpose()
    }
    pub fn initialize(&self, source: &Source) -> Result<()> {
        let root = local::canonical_root(&source.local_root)?;
        if self.directory.starts_with(&root) || root.starts_with(&self.directory) {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "Adoption metadata must stay outside the sync tree.",
            ));
        }
        local::Exclusions::new(&source.excludes)?;
        self.verify_local_root(source)?;
        if let Some(old) = self.get::<Source>("source")? {
            if old.account_email != source.account_email
                || old.local_root != source.local_root
                || old.remote_root_id != source.remote_root_id
                || old.root_identity != source.root_identity
            {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "This manifest belongs to another account or root. Create a separate report.",
                ));
            }
            if old != *source {
                let tx = self.db.unchecked_transaction()?;
                self.set("source", source)?;
                if old.excludes != source.excludes {
                    self.db.execute("DELETE FROM findings", [])?;
                    let mut progress = self.get::<Progress>("progress")?.unwrap_or_default();
                    progress.phase = "pending".into();
                    progress.revision += 1;
                    progress.completed_at = None;
                    progress.error = None;
                    progress.current_path = None;
                    self.set("progress", &progress)?;
                }
                tx.commit()?;
            }
        } else {
            self.set("source", source)?;
            self.set(
                "progress",
                &Progress {
                    phase: "pending".into(),
                    ..Default::default()
                },
            )?;
        }
        Ok(())
    }
    /// For a small folder, use the provider's scoped snapshot instead of listing
    /// the entire account. Its committed change cursor supplies the same boundary.
    pub async fn inventory_folder(&self, provider: &dyn Provider) -> Result<()> {
        let source = self.source()?;
        if !provider
            .identity()
            .await?
            .email
            .eq_ignore_ascii_case(&source.account_email)
        {
            return Err(Error::new(
                ErrorCode::Authentication,
                "The adoption account changed.",
            ));
        }
        let remote =
            crate::remote::snapshot(provider, &source.remote_root_id, &source.excludes, &[])
                .await?;
        let tx = self.db.unchecked_transaction()?;
        self.db.execute("DELETE FROM remote", [])?;
        self.db.execute("DELETE FROM edges", [])?;
        let mut count = 0;
        for items in remote.entries.values() {
            for item in items {
                self.remote_item(item)?;
                count += 1;
            }
        }
        self.set(
            "cursor",
            &remote.cursor.ok_or_else(|| {
                Error::new(
                    ErrorCode::IncompleteScan,
                    "The scoped snapshot did not finish.",
                )
            })?,
        )?;
        self.set("remote_done", &true)?;
        self.set("quiet_listing", &true)?;
        let mut progress = self.get::<Progress>("progress")?.unwrap_or_default();
        progress.remote_items = count;
        self.set("progress", &progress)?;
        tx.commit()?;
        Ok(())
    }
    fn verify_local_root(&self, source: &Source) -> Result<()> {
        let root = local::canonical_root(&source.local_root)?;
        if root != source.local_root
            || local::identity(&fs::metadata(&root)?)
                .unwrap_or_else(|| root.to_string_lossy().into_owned())
                != source.root_identity
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "The adoption root changed. Restore its original location before resuming.",
            ));
        }
        Ok(())
    }
    fn issue(&self, path: &str, reason: &str, side: &str) -> Result<()> {
        self.db.execute("INSERT INTO issues VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET reason=excluded.reason,side=excluded.side", params![path,reason,side])?;
        Ok(())
    }
    fn remote_item(&self, item: &RemoteItem) -> Result<()> {
        if item.trashed {
            return self.mark_remote_unavailable(&item.id);
        }
        self.db
            .execute("DELETE FROM unavailable WHERE id=?1", [&item.id])?;
        self.db
            .execute("DELETE FROM edges WHERE id=?1", [&item.id])?;
        if item.trashed {
            self.db
                .execute("DELETE FROM remote WHERE id=?1", [&item.id])?;
        } else {
            self.db.execute(
                "INSERT INTO remote VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET json=excluded.json",
                params![item.id, serde_json::to_string(item)?],
            )?;
            for parent in &item.parents {
                self.db.execute(
                    "INSERT OR IGNORE INTO edges VALUES(?1,?2)",
                    params![parent, item.id],
                )?;
            }
        }
        Ok(())
    }
    fn mark_remote_unavailable(&self, id: &str) -> Result<()> {
        let json: Option<String> = self
            .db
            .query_row("SELECT json FROM remote WHERE id=?1", [id], |r| r.get(0))
            .optional()?;
        if let Some(json) = json {
            let mut item: RemoteItem = serde_json::from_str(&json)?;
            item.trashed = true;
            self.db.execute("INSERT INTO unavailable VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET json=excluded.json",params![id,serde_json::to_string(&item)?])?;
            // Retain the last known graph position as an unresolved comparison.
            self.db.execute(
                "UPDATE remote SET json=?1 WHERE id=?2",
                params![serde_json::to_string(&item)?, id],
            )?;
        }
        Ok(())
    }
    fn invalidate_remote(&self) -> Result<()> {
        let tx = self.db.unchecked_transaction()?;
        self.db.execute_batch("DELETE FROM remote; DELETE FROM edges; DELETE FROM paths; DELETE FROM state WHERE key IN ('page','cursor','quiet_listing');")?;
        self.set("remote_done", &false)?;
        tx.commit()?;
        Ok(())
    }
    async fn remote_scan(
        &self,
        provider: &dyn Provider,
        p: &mut Progress,
        cancel: &CancellationToken,
    ) -> Result<()> {
        // files.list can shift when entries are added or removed. A change feed
        // merge alone cannot prove an unchanged item was not skipped by a page.
        // Require a complete quiet listing pass before declaring the report ready.
        p.phase = "drive_inventory".into();
        p.current_path = None;
        self.set("progress", p)?;
        for _ in 0..3 {
            if self.get::<String>("cursor")?.is_none() {
                p.remote_items = 0;
                p.drive_passes += 1;
                self.set("cursor", &provider.start_cursor().await?)?;
            }
            if p.drive_passes == 0 {
                p.drive_passes = 1;
            }
            if !self.get::<bool>("remote_done")?.unwrap_or(false) {
                self.set("progress", p)?;
                let mut page = self.get::<Option<String>>("page")?.flatten();
                loop {
                    cancelled(cancel)?;
                    let response = tokio::select! { _ = cancel.cancelled() => { return cancelled(cancel); }, response = provider.inventory_page(page.as_deref()) => response };
                    let response = match response {
                        Err(e) if page.is_some() && e.code == ErrorCode::InvalidConfig => {
                            self.invalidate_remote()?;
                            p.remote_items = 0;
                            return Err(Error::new(
                                ErrorCode::IncompleteScan,
                                "The saved listing page expired. Resume starts a fresh Drive inventory and keeps local checksums.",
                            ));
                        }
                        other => other?,
                    };
                    if page.is_some() && response.next == page {
                        self.invalidate_remote()?;
                        p.remote_items = 0;
                        return Err(Error::new(
                            ErrorCode::IncompleteScan,
                            "Drive repeated an inventory page. Resume starts a fresh listing.",
                        ));
                    }
                    let tx = self.db.unchecked_transaction()?;
                    for item in &response.items {
                        self.remote_item(item)?;
                    }
                    p.remote_items += response.items.len() as u64;
                    page = response.next;
                    self.set("page", &page)?;
                    self.set("remote_done", &page.is_none())?;
                    self.set("progress", p)?;
                    tx.commit()?;
                    if page.is_none() {
                        break;
                    }
                }
            }
            let changed = self.catch_up_or_reset(provider, cancel).await?;
            if !changed {
                self.set("quiet_listing", &true)?;
                return Ok(());
            }
            if self.get::<bool>("quiet_listing")?.unwrap_or_default() {
                // The listing was already verified on a previous run. Applying its
                // feed is safe; activation still performs a fresh scoped inventory.
                return Ok(());
            }
            self.invalidate_remote()?;
            p.remote_items = 0;
            self.set("progress", p)?;
        }
        Err(Error::new(
            ErrorCode::IncompleteScan,
            "Drive changed during repeated inventory passes. Resume after it settles; local checkpoints remain saved.",
        ))
    }
    async fn catch_up_or_reset(
        &self,
        provider: &dyn Provider,
        cancel: &CancellationToken,
    ) -> Result<bool> {
        let result = self.catch_up(provider, cancel).await;
        if result
            .as_ref()
            .is_err_and(|e| e.code == ErrorCode::IncompleteScan)
        {
            self.invalidate_remote()?;
        }
        result
    }
    async fn catch_up(&self, provider: &dyn Provider, cancel: &CancellationToken) -> Result<bool> {
        let mut cursor = self.get::<String>("cursor")?.ok_or_else(|| {
            Error::new(
                ErrorCode::IncompleteScan,
                "The adoption change cursor is missing.",
            )
        })?;
        let mut seen = BTreeSet::new();
        let mut changed = false;
        loop {
            cancelled(cancel)?;
            if !seen.insert(cursor.clone()) {
                return Err(Error::new(
                    ErrorCode::IncompleteScan,
                    "The provider repeated a change page.",
                ));
            }
            let response = tokio::select! { _ = cancel.cancelled() => { return cancelled(cancel).map(|_|false); }, response = provider.changes(&cursor) => response? };
            let tx = self.db.unchecked_transaction()?;
            changed |= !response.changes.is_empty();
            for change in response.changes {
                if change.removed {
                    // Removed can mean lost access. Adoption never infers a deletion.
                    self.mark_remote_unavailable(&change.id)?;
                } else if let Some(item) = change.item {
                    self.remote_item(&item)?;
                } else {
                    return Err(Error::new(
                        ErrorCode::IncompleteScan,
                        "A Drive change has no comparison metadata.",
                    ));
                }
            }
            let next = response.next;
            cursor = next.clone().or(response.new_cursor).ok_or_else(|| {
                Error::new(
                    ErrorCode::IncompleteScan,
                    "Drive did not finish its change inventory.",
                )
            })?;
            self.set("cursor", &cursor)?;
            tx.commit()?;
            if next.is_none() {
                return Ok(changed);
            }
        }
    }
    fn resolve_paths(&self, source: &Source, cancel: &CancellationToken) -> Result<()> {
        let exclusions = local::Exclusions::new(&source.excludes)?;
        let tx = self.db.unchecked_transaction()?;
        let mut missing = self.db.prepare("SELECT id,json FROM unavailable")?;
        for row in
            missing.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        {
            let (id, json) = row?;
            let present: bool = self.db.query_row(
                "SELECT EXISTS(SELECT 1 FROM remote WHERE id=?1)",
                [&id],
                |r| r.get(0),
            )?;
            if !present {
                let item: RemoteItem = serde_json::from_str(&json)?;
                self.db
                    .execute("INSERT INTO remote VALUES(?1,?2)", params![id, json])?;
                for parent in item.parents {
                    self.db.execute(
                        "INSERT OR IGNORE INTO edges VALUES(?1,?2)",
                        params![parent, id],
                    )?;
                }
            }
        }
        self.db.execute("DELETE FROM paths", [])?;
        self.db
            .execute("DELETE FROM issues WHERE side='drive'", [])?;
        let mut todo = VecDeque::from([(String::new(), source.remote_root_id.clone())]);
        let mut visited = BTreeSet::from([source.remote_root_id.clone()]);
        while let Some((prefix, id)) = todo.pop_front() {
            cancelled(cancel)?;
            let mut statement = self.db.prepare("SELECT r.json FROM edges e JOIN remote r ON r.id=e.id WHERE e.parent=?1 ORDER BY r.id")?;
            let rows = statement.query_map([id], |r| r.get::<_, String>(0))?;
            for row in rows {
                let item: RemoteItem = serde_json::from_str(&row?)?;
                let path = if prefix.is_empty() {
                    item.name.clone()
                } else {
                    format!("{prefix}/{}", item.name)
                };
                if exclusions.excludes(&path) {
                    self.issue(&path, "Excluded by reviewed rules", "drive")?;
                    continue;
                }
                if item.trashed {
                    self.issue(&path,"Drive removal or access loss occurred during inventory; confirm this difference before adoption","drive")?;
                    continue;
                }
                if local::validate_relative(&item.name).is_err() || item.name.contains('/') {
                    self.issue(
                        if prefix.is_empty() { &path } else { &prefix },
                        "Unsupported Drive name; review portable naming before activation",
                        "drive",
                    )?;
                    continue;
                }
                self.db.execute(
                    "INSERT OR IGNORE INTO paths VALUES(?1,?2)",
                    params![path, item.id],
                )?;
                if item.kind == ItemKind::Folder {
                    if !visited.insert(item.id.clone()) {
                        self.issue(
                            &path,
                            "Folder alias or cycle requires manual review",
                            "drive",
                        )?;
                    } else {
                        todo.push_back((path, item.id));
                    }
                }
            }
        }
        tx.commit()?;
        Ok(())
    }
    fn cached_local(&self, path: &str) -> Result<Option<LocalEntry>> {
        let s: Option<String> = self
            .db
            .query_row("SELECT json FROM local WHERE path=?1", [path], |r| r.get(0))
            .optional()?;
        s.map(|s| serde_json::from_str(&s).map_err(Into::into))
            .transpose()
    }
    fn local_scan(
        &self,
        source: &Source,
        p: &mut Progress,
        cancel: &CancellationToken,
    ) -> Result<()> {
        self.verify_local_root(source)?;
        let exclusions = local::Exclusions::new(&source.excludes)?;
        let run = self.get::<u64>("local_run")?.unwrap_or(0) + 1;
        self.set("local_run", &run)?;
        self.db
            .execute("DELETE FROM issues WHERE side='local'", [])?;
        p.phase = "local_inventory".into();
        p.local_items = 0;
        p.current_path = None;
        self.set("progress", p)?;
        let mut todo = vec![String::new()];
        let mut tx = self.db.unchecked_transaction()?;
        while let Some(dir) = todo.pop() {
            if cancel.is_cancelled() {
                self.set("progress", p)?;
                tx.commit()?;
                return cancelled(cancel);
            }
            let full = if dir.is_empty() {
                source.local_root.clone()
            } else {
                local::safe_join(&source.local_root, &dir)?
            };
            let before = fs::symlink_metadata(&full)?;
            if before.file_type().is_symlink() || !before.is_dir() {
                return Err(Error::new(
                    ErrorCode::IncompleteScan,
                    "A directory changed during adoption. Resume to recheck.",
                ));
            }
            let children = match fs::read_dir(&full) {
                Ok(c) => c,
                Err(_) => {
                    self.issue(
                        &dir,
                        "Directory cannot be read; its entire subtree remains protected",
                        "local",
                    )?;
                    continue;
                }
            };
            for child in children {
                if cancel.is_cancelled() {
                    self.set("progress", p)?;
                    tx.commit()?;
                    return cancelled(cancel);
                }
                let child = child?;
                let name = match child.file_name().into_string() {
                    Ok(n) => n,
                    Err(_) => {
                        self.issue(
                            &dir,
                            "Non UTF-8 filename; its parent remains protected",
                            "local",
                        )?;
                        continue;
                    }
                };
                let path = if dir.is_empty() {
                    name
                } else {
                    format!("{dir}/{name}")
                };
                p.current_path = Some(path.clone());
                if exclusions.excludes(&path) {
                    self.issue(&path, "Excluded by reviewed rules", "local")?;
                    continue;
                }
                if local::validate_relative(&path).is_err() {
                    self.issue(
                        &path,
                        "Unsupported local name; review before activation",
                        "local",
                    )?;
                    continue;
                }
                let meta = match fs::symlink_metadata(child.path()) {
                    Ok(m) => m,
                    Err(_) => {
                        self.issue(&path, "File disappeared or cannot be read", "local")?;
                        continue;
                    }
                };
                if meta.file_type().is_symlink() || !meta.is_file() && !meta.is_dir() {
                    self.issue(
                        &path,
                        "Links and special files stay outside adoption",
                        "local",
                    )?;
                    continue;
                }
                let kind = if meta.is_dir() {
                    ItemKind::Folder
                } else {
                    ItemKind::File
                };
                let modified_ns = local::modified(&meta)?;
                let file_identity = local::identity(&meta);
                let old = self.cached_local(&path)?;
                let fingerprint = if kind == ItemKind::Folder {
                    None
                } else if let Some(old) = old.filter(|o| {
                    o.kind == kind
                        && o.modified_ns == modified_ns
                        && o.file_identity == file_identity
                        && o.fingerprint.as_ref().is_some_and(|f| f.size == meta.len())
                }) {
                    p.reused_hashes += 1;
                    old.fingerprint
                } else {
                    if meta.len() >= 16 * 1024 * 1024 {
                        self.set("progress", p)?;
                        tx.commit()?;
                        tx = self.db.unchecked_transaction()?;
                    }
                    match local::fingerprint_cancellable(&child.path(), cancel) {
                        Ok(f) => {
                            p.hashed_bytes += f.size;
                            Some(f)
                        }
                        Err(e) if e.code == ErrorCode::Cancelled => {
                            self.set("progress", p)?;
                            tx.commit()?;
                            return Err(e);
                        }
                        Err(e) => {
                            self.issue(&path, &e.message, "local")?;
                            continue;
                        }
                    }
                };
                let entry = LocalEntry {
                    path: path.clone(),
                    kind,
                    fingerprint,
                    modified_ns,
                    file_identity,
                };
                self.db.execute("INSERT INTO local VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET json=excluded.json,seen=excluded.seen", params![path,serde_json::to_string(&entry)?,run as i64])?;
                p.local_items += 1;
                if kind == ItemKind::Folder {
                    todo.push(path);
                }
                if p.local_items.is_multiple_of(256) {
                    self.set("progress", p)?;
                    tx.commit()?;
                    tx = self.db.unchecked_transaction()?;
                }
            }
            let after = fs::symlink_metadata(&full)?;
            if local::identity(&before) != local::identity(&after)
                || local::modified(&before)? != local::modified(&after)?
            {
                self.set("progress", p)?;
                tx.commit()?;
                return Err(Error::new(
                    ErrorCode::IncompleteScan,
                    "A directory changed during inventory. Resume keeps verified hashes and rechecks the tree.",
                ));
            }
        }
        self.db
            .execute("DELETE FROM local WHERE seen<>?1", [run as i64])?;
        self.set("progress", p)?;
        tx.commit()?;
        self.verify_local_root(source)
    }
    fn verify_local_entries(&self, source: &Source, cancel: &CancellationToken) -> Result<()> {
        let mut statement = self.db.prepare("SELECT json FROM local ORDER BY path")?;
        for row in statement.query_map([], |r| r.get::<_, String>(0))? {
            cancelled(cancel)?;
            let entry: LocalEntry = serde_json::from_str(&row?)?;
            let path = source.local_root.join(&entry.path);
            let meta = fs::symlink_metadata(path)?;
            if meta.file_type().is_symlink()
                || local::identity(&meta) != entry.file_identity
                || local::modified(&meta)? != entry.modified_ns
                || entry
                    .fingerprint
                    .as_ref()
                    .is_some_and(|f| f.size != meta.len())
            {
                return Err(Error::new(
                    ErrorCode::IncompleteScan,
                    "Local content changed before matching. Resume rechecks stale comparisons.",
                ));
            }
        }
        self.verify_local_root(source)
    }
    fn match_entries(&self, p: &mut Progress, cancel: &CancellationToken) -> Result<()> {
        p.phase = "matching".into();
        self.set("progress", p)?;
        let tx = self.db.unchecked_transaction()?;
        self.db.execute("DELETE FROM findings", [])?;
        let mut blocked: BTreeMap<String, String> = BTreeMap::new();
        let mut statement = self.db.prepare("SELECT path,reason FROM issues UNION ALL SELECT path,'Duplicate Drive names require manual review' FROM paths GROUP BY path HAVING count(*)>1 ORDER BY path")?;
        for row in
            statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        {
            let (path, reason) = row?;
            blocked.insert(path, reason);
        }
        let mut statement = self.db.prepare("SELECT path FROM local UNION SELECT path FROM paths UNION SELECT path FROM issues ORDER BY path")?;
        let mut remote_statement = self.db.prepare(
            "SELECT r.json FROM paths p JOIN remote r ON r.id=p.id WHERE p.path=?1 ORDER BY r.id",
        )?;
        for row in statement.query_map([], |r| r.get::<_, String>(0))? {
            cancelled(cancel)?;
            let path = row?;
            let local = self.cached_local(&path)?;
            let remote: Vec<RemoteItem> = remote_statement
                .query_map([&path], |r| r.get::<_, String>(0))?
                .map(|r| Ok(serde_json::from_str(&r?)?))
                .collect::<Result<_>>()?;
            if local.as_ref().is_some_and(|l| l.kind == ItemKind::Folder)
                && remote.len() == 1
                && remote[0].kind != ItemKind::Folder
            {
                blocked.insert(
                    path.clone(),
                    "A folder has a different item type on Drive; its descendants require review"
                        .into(),
                );
            }
            let mut prefix = path.as_str();
            let blocked_reason = loop {
                if let Some(reason) = blocked.get(prefix) {
                    break Some(reason.clone());
                }
                if let Some((parent, _)) = prefix.rsplit_once('/') {
                    prefix = parent;
                } else {
                    break blocked.get("").cloned();
                }
            };
            // Existing InSync links are protected until the native-document policy is implemented.
            let link = path.rsplit_once('.').is_some_and(|(_, ext)| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "gdoc"
                        | "gsheet"
                        | "gslides"
                        | "gdraw"
                        | "gform"
                        | "gmap"
                        | "gsite"
                        | "desktop"
                )
            });
            let (status, reason) = if let Some(reason) = blocked_reason {
                (
                    if reason.starts_with("Excluded") {
                        "excluded"
                    } else {
                        "unresolved"
                    },
                    reason,
                )
            } else if link
                || remote
                    .iter()
                    .any(|r| matches!(r.kind, ItemKind::NativeDocument | ItemKind::Shortcut))
            {
                ("protected","Native documents, shortcuts and existing link files require the Phase 9 file policy".into())
            } else {
                match (local.as_ref(), remote.as_slice()) {
                    (Some(l), [r]) if r.matches_local(l) => (
                        "matched",
                        "Equivalent content; adopt the existing Drive ID without transfer".into(),
                    ),
                    (Some(_), [_]) => (
                        "unresolved",
                        "Initial content or item type differs; preserve both sides for review"
                            .into(),
                    ),
                    (Some(l), []) => (
                        "upload",
                        if l.kind == ItemKind::Folder {
                            "Would create a Drive folder"
                        } else {
                            "Would upload local content"
                        }
                        .into(),
                    ),
                    (None, [r]) => (
                        "download",
                        if r.kind == ItemKind::Folder {
                            "Would create a local folder"
                        } else {
                            "Would download Drive content"
                        }
                        .into(),
                    ),
                    _ => (
                        "unresolved",
                        "Unavailable or ambiguous comparison; preserve both sides".into(),
                    ),
                }
            };
            let finding = Finding {
                path: path.clone(),
                status: status.into(),
                reason,
                size_bytes: local
                    .as_ref()
                    .and_then(|l| l.fingerprint.as_ref())
                    .or_else(|| remote.first().and_then(|r| r.fingerprint.as_ref()))
                    .map(|f| f.size),
                remote_id: (remote.len() == 1).then(|| remote[0].id.clone()),
                kind: local
                    .as_ref()
                    .map(|l| l.kind)
                    .or_else(|| remote.first().map(|r| r.kind)),
            };
            self.db.execute(
                "INSERT INTO findings VALUES(?1,?2,?3)",
                params![path, status, serde_json::to_string(&finding)?],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    pub async fn run(&self, provider: &dyn Provider, cancel: &CancellationToken) -> Result<()> {
        let source = self.source()?;
        let mut p = self.get::<Progress>("progress")?.unwrap_or_default();
        p.error = None;
        p.completed_at = None;
        p.revision += 1;
        self.set("progress", &p)?;
        let result = async {
            self.verify_local_root(&source)?;
            if !provider
                .identity()
                .await?
                .email
                .eq_ignore_ascii_case(&source.account_email)
            {
                return Err(Error::new(
                    ErrorCode::Authentication,
                    "The Drive account differs from this adoption manifest.",
                ));
            }
            let root = provider.get(&source.remote_root_id).await?;
            if root.id != source.remote_root_id || root.trashed || root.kind != ItemKind::Folder {
                return Err(Error::new(
                    ErrorCode::UnsafePath,
                    "The selected Drive root is unavailable or changed.",
                ));
            }
            // Preserve useful local work even if a busy Drive cannot finish a
            // quiet listing. Final verification below rechecks these stamps.
            self.local_scan(&source, &mut p, cancel)?;
            self.remote_scan(provider, &mut p, cancel).await?;
            p.phase = "rechecking".into();
            self.set("progress", &p)?;
            self.catch_up_or_reset(provider, cancel).await?;
            self.resolve_paths(&source, cancel)?;
            self.verify_local_entries(&source, cancel)?;
            let root_after = provider.get(&source.remote_root_id).await?;
            if root_after.trashed
                || root_after.name != root.name
                || root_after.parents != root.parents
            {
                return Err(Error::new(
                    ErrorCode::IncompleteScan,
                    "The Drive root moved during adoption. Resume to recheck.",
                ));
            }
            self.match_entries(&mut p, cancel)?;
            p.phase = "ready".into();
            p.completed_at = Some(crate::executor::now());
            p.current_path = None;
            Ok(())
        }
        .await;
        if let Err(e) = &result {
            p.phase = if e.code == ErrorCode::Cancelled {
                "cancelled"
            } else {
                "failed"
            }
            .into();
            p.error = Some(e.clone());
        }
        self.set("progress", &p)?;
        if result.is_ok() {
            let summary = self.report(None, "", 0, 50)?;
            self.write_summary(&summary)?;
        }
        result
    }
    fn write_summary(&self, summary: &Report) -> Result<()> {
        use std::io::Write;
        let temporary = self
            .directory
            .join(format!("summary-{}.tmp", uuid::Uuid::new_v4()));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| -> Result<()> {
            let mut file = options.open(&temporary)?;
            file.write_all(&serde_json::to_vec_pretty(summary)?)?;
            file.sync_all()?;
            fs::rename(&temporary, self.directory.join("summary.json"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
    pub fn report(
        &self,
        status: Option<&str>,
        name: &str,
        offset: u64,
        limit: u64,
    ) -> Result<Report> {
        let mut counts = BTreeMap::new();
        let mut statement = self
            .db
            .prepare("SELECT status,count(*) FROM findings GROUP BY status")?;
        for row in statement.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
        })? {
            let (k, v) = row?;
            counts.insert(k, v);
        }
        let matching=self.db.query_row("SELECT count(*) FROM findings WHERE (?1 IS NULL OR status=?1) AND instr(lower(path),lower(?2))>0",params![status,name],|r|r.get::<_,i64>(0))? as u64;
        let mut statement=self.db.prepare("SELECT json FROM findings WHERE (?1 IS NULL OR status=?1) AND instr(lower(path),lower(?2))>0 ORDER BY path LIMIT ?3 OFFSET ?4")?;
        let findings = statement
            .query_map(
                params![
                    status,
                    name,
                    limit.clamp(1, 200) as i64,
                    offset.min(i64::MAX as u64) as i64
                ],
                |r| r.get::<_, String>(0),
            )?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect::<Result<_>>()?;
        Ok(Report {
            source: self.source()?,
            progress: self.get("progress")?.unwrap_or_default(),
            counts,
            findings,
            matching,
            offset,
            directory: self.directory.clone(),
        })
    }
    /// A selected folder must have a unique, ordinary ID on both sides.
    pub fn scope(&self, path: &str, revision: u64) -> Result<PairConfig> {
        local::validate_relative(path)?;
        let source = self.source()?;
        let p = self.get::<Progress>("progress")?.unwrap_or_default();
        if p.phase != "ready" || p.revision != revision {
            return Err(Error::new(
                ErrorCode::Conflict,
                "The adoption report changed or is incomplete. Review a finished report.",
            ));
        }
        let finding: String = self
            .db
            .query_row("SELECT json FROM findings WHERE path=?1", [path], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::InvalidConfig,
                    "Choose a folder in the adoption report.",
                )
            })?;
        let finding: Finding = serde_json::from_str(&finding)?;
        let local = self.cached_local(path)?.ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidConfig,
                "The selected folder is not present locally.",
            )
        })?;
        if finding.status != "matched" || local.kind != ItemKind::Folder {
            return Err(Error::new(
                ErrorCode::Conflict,
                "Only an unambiguous existing folder can be selected. Review its discrepancy first.",
            ));
        }
        let root = local::safe_join(&source.local_root, path)?;
        let mut excludes = Vec::new();
        for rule in &source.excludes {
            if let Some(relative) = rule.strip_prefix(&format!("{}/", literal_glob(path))) {
                excludes.push(relative.into());
            } else if simple_global_rule(rule) {
                excludes.push(rule.clone());
            } else {
                // Do not silently lose a glob crossing the selected folder's
                // ancestors. More general selective-sync rules come in Phase 9.
                let mut literal = String::new();
                let mut escaped = false;
                let mut wildcard = false;
                for character in rule.chars() {
                    if escaped {
                        literal.push(character);
                        escaped = false;
                    } else if character == '\\' {
                        escaped = true;
                    } else if matches!(character, '*' | '?' | '[' | '{') {
                        wildcard = true;
                        break;
                    } else {
                        literal.push(character);
                    }
                }
                if wildcard
                    && (path.starts_with(&literal) || literal.starts_with(&format!("{path}/")))
                {
                    return Err(Error::new(
                        ErrorCode::Unsupported,
                        "An exclusion spans this folder's ancestors. Select a higher folder or define a scope-relative exclusion before activation.",
                    ));
                }
            }
        }
        for extension in [
            "gdoc", "gsheet", "gslides", "gdraw", "gform", "gmap", "gsite", "desktop",
        ] {
            let letters: String = extension
                .chars()
                .map(|c| format!("[{c}{}]", c.to_ascii_uppercase()))
                .collect();
            excludes.push(format!("**/*.{letters}"));
        }
        let mut statement = self.db.prepare("SELECT path FROM issues ORDER BY path")?;
        for row in statement.query_map([], |r| r.get::<_, String>(0))? {
            if let Some(relative) = row?.strip_prefix(&format!("{path}/")) {
                excludes.push(literal_glob(relative));
            }
        }
        let mut statement = self.db.prepare(
            "SELECT path FROM findings WHERE status IN ('excluded','protected') ORDER BY path",
        )?;
        for row in statement.query_map([], |r| r.get::<_, String>(0))? {
            if let Some(relative) = row?.strip_prefix(&format!("{path}/")) {
                excludes.push(literal_glob(relative));
            }
        }
        excludes.sort();
        excludes.dedup();
        Ok(PairConfig {
            id: format!("adopted-{}", uuid::Uuid::new_v4()),
            account_email: source.account_email,
            local_root: root,
            remote_root_id: finding.remote_id.ok_or_else(|| {
                Error::new(
                    ErrorCode::Conflict,
                    "The selected folder has no unique Drive identity.",
                )
            })?,
            remote_root_name: path.rsplit('/').next().unwrap_or(path).into(),
            root_identity: local.file_identity.ok_or_else(|| {
                Error::new(
                    ErrorCode::Unsupported,
                    "This filesystem cannot identify the selected root.",
                )
            })?,
            excludes,
            enabled: false,
            poll_secs: 60,
            deletion_limit: 25,
            test_only: false,
        })
    }
    pub fn review_scope(&self, path: &str, revision: u64) -> Result<serde_json::Value> {
        let pair = self.scope(path, revision)?;
        let prefix = format!("{path}/");
        let mut counts = BTreeMap::new();
        let mut statement = self.db.prepare(
            "SELECT status,count(*) FROM findings WHERE substr(path,1,?1)=?2 GROUP BY status",
        )?;
        for row in statement.query_map(params![prefix.chars().count() as i64, prefix], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
        })? {
            let (k, v) = row?;
            counts.insert(k, v);
        }
        Ok(serde_json::json!({"scope":path,"revision":revision,"pair":pair,"counts":counts}))
    }
    pub fn verify_scope(
        &self,
        scope: &str,
        pair: &PairConfig,
        local: &LocalInventory,
        remote: &RemoteInventory,
    ) -> Result<()> {
        let exclusions = local::Exclusions::new(&pair.excludes)?;
        let mut expected_local = BTreeMap::new();
        let mut expected_remote: BTreeMap<String, Vec<RemoteItem>> = BTreeMap::new();
        let prefix = format!("{scope}/");
        let mut statement = self
            .db
            .prepare("SELECT path,json FROM local WHERE substr(path,1,?1)=?2 ORDER BY path")?;
        for row in statement.query_map(params![prefix.chars().count() as i64, prefix], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (path, json) = row?;
            let relative = path.strip_prefix(&prefix).unwrap();
            if !exclusions.excludes(relative) {
                let mut entry: LocalEntry = serde_json::from_str(&json)?;
                entry.path = relative.into();
                expected_local.insert(relative.to_owned(), entry);
            }
        }
        let mut statement=self.db.prepare("SELECT p.path,r.json FROM paths p JOIN remote r ON r.id=p.id WHERE substr(p.path,1,?1)=?2 ORDER BY p.path,r.id")?;
        for row in statement.query_map(params![prefix.chars().count() as i64, prefix], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (path, json) = row?;
            let relative = path.strip_prefix(&prefix).unwrap();
            if !exclusions.excludes(relative) {
                expected_remote
                    .entry(relative.into())
                    .or_default()
                    .push(serde_json::from_str(&json)?);
            }
        }
        let local_same = expected_local.len() == local.entries.len()
            && local.entries.iter().all(|(path, l)| {
                // The activation scan rehashes content. Metadata-only touches
                // retain equivalent bytes and commit the freshly observed stamp.
                expected_local.get(path).is_some_and(|e| e.content_eq(l))
            });
        let remote_same = expected_remote.len() == remote.entries.len()
            && remote.entries.iter().all(|(path, items)| {
                expected_remote.get(path).is_some_and(|expected| {
                    expected.len() == items.len()
                        && items.iter().all(|r| {
                            expected.iter().any(|e| {
                                e.id == r.id
                                    && e.content_eq(r)
                                    && (e.version == r.version
                                        || e.kind == ItemKind::Folder
                                        || e.fingerprint.is_some())
                                    && e.parents == r.parents
                                    && e.name == r.name
                            })
                        })
                })
            });
        if !local_same || !remote_same || local.root_identity != pair.root_identity {
            return Err(Error::new(
                ErrorCode::Conflict,
                "The selected folder changed since this report. Refresh the report and review its new proposed changes.",
            ));
        }
        Ok(())
    }
    /// A folder can move while all its descendants retain their parent IDs.
    /// Check the selected root's own location against the reviewed manifest.
    pub fn verify_remote_root(&self, path: &str, observed: &RemoteItem) -> Result<()> {
        let expected: Option<String> = self
            .db
            .query_row(
                "SELECT r.json FROM paths p JOIN remote r ON r.id=p.id WHERE p.path=?1 AND r.id=?2",
                params![path, observed.id],
                |r| r.get(0),
            )
            .optional()?;
        let valid = expected
            .map(|json| -> Result<bool> {
                let expected: RemoteItem = serde_json::from_str(&json)?;
                Ok(!observed.trashed
                    && observed.kind == ItemKind::Folder
                    && expected.kind == ItemKind::Folder
                    && observed.name == expected.name
                    && observed.parents == expected.parents)
            })
            .transpose()?
            .unwrap_or(false);
        if !valid {
            return Err(Error::new(
                ErrorCode::Conflict,
                "The reviewed Drive folder moved or changed. Refresh adoption before activation.",
            ));
        }
        Ok(())
    }
    pub async fn verify_remote_location(&self, provider: &dyn Provider, path: &str) -> Result<()> {
        let mut prefix = String::new();
        for part in path.split('/') {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(part);
            let ids: Vec<String> = self
                .db
                .prepare("SELECT id FROM paths WHERE path=?1")?
                .query_map([&prefix], |r| r.get(0))?
                .collect::<std::result::Result<_, _>>()?;
            if ids.len() != 1 {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "A reviewed Drive ancestor is ambiguous. Refresh adoption.",
                ));
            }
            self.verify_remote_root(&prefix, &provider.get(&ids[0]).await?)?;
        }
        Ok(())
    }
    pub fn backup(&self, destination: &Path) -> Result<()> {
        profile::private_directory(destination)?;
        let target = destination.join("manifest.sqlite3");
        if target.exists() {
            return Err(Error::new(
                ErrorCode::Conflict,
                "The adoption backup already exists.",
            ));
        }
        self.db
            .execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])?;
        Ok(())
    }
}

// A recursive literal name or a single leading wildcard with a literal suffix
// retains its meaning below a selected root. More complex ancestor-spanning
// globs must be defined relative to that root rather than silently weakened.
fn simple_global_rule(rule: &str) -> bool {
    let recursive = rule.starts_with("**/");
    let name = rule.strip_prefix("**/").unwrap_or(rule);
    let wildcard = name.starts_with('*');
    let suffix = name.strip_prefix('*').unwrap_or(name);
    (recursive || wildcard)
        && !suffix.is_empty()
        && !suffix
            .chars()
            .any(|c| matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '\\' | '/'))
}
