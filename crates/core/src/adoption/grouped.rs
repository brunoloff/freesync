//! Verify small parent-scoped inventories instead of requiring a quiet account.
use super::*;
use crate::provider::Page;

#[derive(Serialize, Deserialize)]
pub(super) struct Delta {
    pub before: Option<RemoteItem>,
    pub after: Option<RemoteItem>,
    pub removed: bool,
}
impl Delta {
    fn affects_folders(&self) -> bool {
        self.before
            .as_ref()
            .is_some_and(|item| item.kind == ItemKind::Folder)
            || self
                .after
                .as_ref()
                .is_some_and(|item| item.kind == ItemKind::Folder)
            || self.removed && self.before.is_none()
    }
    fn affects(&self, parents: &[String]) -> bool {
        self.removed && self.before.is_none()
            || [&self.before, &self.after].into_iter().any(|item| {
                item.as_ref()
                    .is_some_and(|item| item.parents.iter().any(|id| parents.contains(id)))
            })
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct Group {
    id: i64,
    parents: Vec<String>,
    page: Option<String>,
    pages: u64,
    complete: bool,
    seen_pages: BTreeSet<String>,
}

impl Manifest {
    fn inventory_deltas(&self) -> Result<Vec<Delta>> {
        let mut statement = self
            .db
            .prepare("SELECT json FROM inventory_deltas ORDER BY seq")?;
        statement
            .query_map([], |r| r.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }
    fn save_group(&self, group: &Group, verified: bool) -> Result<()> {
        self.db.execute(
            "INSERT INTO inventory_groups VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET json=excluded.json,verified=excluded.verified",
            params![group.id, serde_json::to_string(group)?, verified],
        )?;
        Ok(())
    }
    fn group(&self, id: i64) -> Result<Group> {
        let json: String =
            self.db
                .query_row("SELECT json FROM inventory_groups WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
        Ok(serde_json::from_str(&json)?)
    }
    fn enqueue_folders(&self) -> Result<()> {
        let mut statement = self.db.prepare("SELECT c.id FROM inventory_folder_candidates c JOIN remote r ON r.id=c.id LEFT JOIN inventory_parents p ON p.id=c.id WHERE p.id IS NULL AND json_extract(r.json,'$.kind')='folder' AND json_extract(r.json,'$.trashed')=0 ORDER BY c.id")?;
        let folders = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let next: i64 = self.db.query_row(
            "SELECT COALESCE(max(id),0)+1 FROM inventory_groups",
            [],
            |r| r.get(0),
        )?;
        let tx = self.db.unchecked_transaction()?;
        for (id, parents) in (next..).zip(folders.chunks(64)) {
            let group = Group {
                id,
                parents: parents.to_vec(),
                page: None,
                pages: 0,
                complete: false,
                seen_pages: BTreeSet::new(),
            };
            self.save_group(&group, false)?;
            for parent in parents {
                self.db.execute(
                    "INSERT INTO inventory_parents VALUES(?1,?2)",
                    params![parent, id],
                )?;
            }
        }
        self.set(
            "inventory_folders_total",
            &(self
                .get::<u64>("inventory_folders_total")?
                .unwrap_or_default()
                + folders.len() as u64),
        )?;
        self.db
            .execute("DELETE FROM inventory_folder_candidates", [])?;
        tx.commit()?;
        Ok(())
    }
    fn inventory_metrics(&self, progress: &mut Progress) -> Result<()> {
        progress.remote_items = self
            .db
            .query_row("SELECT count(*) FROM remote", [], |r| r.get::<_, i64>(0))?
            as u64;
        progress.remote_folders_total = self.get("inventory_folders_total")?.unwrap_or_default();
        progress.remote_folders_checked =
            self.get("inventory_folders_checked")?.unwrap_or_default();
        self.set("progress", progress)
    }
    fn clear_group_files(&self, group: &Group) -> Result<()> {
        let mut ids = BTreeSet::new();
        let mut statement = self.db.prepare("SELECT r.id FROM edges e JOIN remote r ON r.id=e.id WHERE e.parent=?1 AND json_extract(r.json,'$.kind')!='folder'")?;
        for parent in &group.parents {
            ids.extend(
                statement
                    .query_map([parent], |r| r.get::<_, String>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?,
            );
        }
        for id in ids {
            self.db.execute("DELETE FROM edges WHERE id=?1", [&id])?;
            self.db.execute("DELETE FROM remote WHERE id=?1", [&id])?;
        }
        Ok(())
    }
    async fn grouped_changes(
        &self,
        provider: &dyn Provider,
        cancel: &CancellationToken,
        record: bool,
    ) -> Result<()> {
        let result = self.consume_changes(provider, cancel, record).await;
        if result
            .as_ref()
            .is_err_and(|error| error.code == ErrorCode::IncompleteScan)
        {
            self.invalidate_remote()?;
        }
        result.map(|_| ())
    }
    async fn folder_census(
        &self,
        provider: &dyn Provider,
        progress: &mut Progress,
        cancel: &CancellationToken,
    ) -> Result<()> {
        if self.get::<bool>("folders_done")?.unwrap_or_default() {
            return Ok(());
        }
        for _ in 0..3 {
            if self.get::<String>("cursor")?.is_none() {
                self.set("cursor", &provider.start_cursor().await?)?;
                progress.drive_passes += 1;
            }
            let mut page = self.get::<Option<String>>("page")?.flatten();
            let mut seen = self
                .get::<BTreeSet<String>>("folder_seen_pages")?
                .unwrap_or_default();
            if !self.get::<bool>("folder_pages_done")?.unwrap_or_default() {
                loop {
                    cancelled(cancel)?;
                    let response = tokio::select! { _ = cancel.cancelled() => return cancelled(cancel), response = provider.inventory_folders(page.as_deref()) => response };
                    let response = match response {
                        Err(error) if error.code == ErrorCode::InvalidConfig && page.is_some() => {
                            self.invalidate_remote()?;
                            return Err(Error::new(
                                ErrorCode::IncompleteScan,
                                "The saved folder page expired. Resume keeps local hashes and restarts the folder census.",
                            ));
                        }
                        other => other?,
                    };
                    if response
                        .next
                        .as_ref()
                        .is_some_and(|next| Some(next) == page.as_ref() || seen.contains(next))
                    {
                        self.invalidate_remote()?;
                        return Err(Error::new(
                            ErrorCode::IncompleteScan,
                            "Drive repeated a folder page. Resume restarts the folder census.",
                        ));
                    }
                    let tx = self.db.unchecked_transaction()?;
                    for item in response.items {
                        if item.kind != ItemKind::Folder {
                            return Err(Error::new(
                                ErrorCode::IncompleteScan,
                                "The folder census contained a different item type.",
                            ));
                        }
                        self.remote_item(&item)?;
                    }
                    if let Some(old) = page {
                        seen.insert(old);
                    }
                    page = response.next;
                    self.set("page", &page)?;
                    self.set("folder_seen_pages", &seen)?;
                    self.set("folder_pages_done", &page.is_none())?;
                    self.set(
                        "folder_census_pages",
                        &(self.get::<u64>("folder_census_pages")?.unwrap_or_default() + 1),
                    )?;
                    self.inventory_metrics(progress)?;
                    tx.commit()?;
                    if page.is_none() {
                        break;
                    }
                }
            }
            self.grouped_changes(provider, cancel, true).await?;
            if self.get::<u64>("folder_census_pages")?.unwrap_or_default() > 1
                && self.inventory_deltas()?.iter().any(Delta::affects_folders)
            {
                self.invalidate_remote()?;
                progress.remote_items = 0;
                self.inventory_metrics(progress)?;
                continue;
            }
            let tx = self.db.unchecked_transaction()?;
            self.set("folders_done", &true)?;
            self.db.execute("DELETE FROM inventory_deltas", [])?;
            tx.commit()?;
            return Ok(());
        }
        Err(Error::new(
            ErrorCode::IncompleteScan,
            "Drive folders kept changing during their census. Resume keeps local hashes and verified work.",
        ))
    }
    async fn fetch_inventory_group(
        provider: &dyn Provider,
        group: Option<Group>,
    ) -> Result<Option<(Group, Page<RemoteItem>)>> {
        let Some(group) = group else {
            return Ok(None);
        };
        let response = provider
            .inventory_children(&group.parents, group.page.as_deref())
            .await?;
        Ok(Some((group, response)))
    }
    pub(super) async fn grouped_remote_scan(
        &self,
        provider: &dyn Provider,
        progress: &mut Progress,
        cancel: &CancellationToken,
    ) -> Result<()> {
        if self.get::<String>("inventory_strategy")?.as_deref() != Some("parent_groups_v1") {
            self.invalidate_remote()?;
            self.set("inventory_strategy", &"parent_groups_v1")?;
        }
        progress.phase = "drive_inventory".into();
        progress.current_path = None;
        self.inventory_metrics(progress)?;
        self.folder_census(provider, progress, cancel).await?;
        let source = self.source()?;
        self.remote_item(&provider.get(&source.remote_root_id).await?)?;
        let mut retries = BTreeMap::<i64, u64>::new();
        loop {
            cancelled(cancel)?;
            let mut active = self
                .get::<Vec<i64>>("active_inventory_groups")?
                .unwrap_or_default();
            if active.is_empty() {
                // Only consume an unrecorded gap after the preceding groups have
                // been verified. An unfinished cohort retains its original cursor.
                self.grouped_changes(provider, cancel, false).await?;
                self.enqueue_folders()?;
                let mut statement = self.db.prepare(
                    "SELECT id FROM inventory_groups WHERE verified=0 ORDER BY id LIMIT 4",
                )?;
                active = statement
                    .query_map([], |r| r.get::<_, i64>(0))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                if active.is_empty() {
                    self.set("remote_done", &true)?;
                    self.set("quiet_listing", &true)?;
                    self.inventory_metrics(progress)?;
                    return Ok(());
                }
                let tx = self.db.unchecked_transaction()?;
                for id in &active {
                    let group = self.group(*id)?;
                    if group.pages == 0 {
                        self.clear_group_files(&group)?;
                    }
                }
                self.set("active_inventory_groups", &active)?;
                self.set("remote_done", &false)?;
                tx.commit()?;
            }
            let pending: Vec<Group> = active
                .iter()
                .map(|id| self.group(*id))
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .filter(|group| !group.complete)
                .collect();
            if !pending.is_empty() {
                let responses = tokio::select! {
                    _ = cancel.cancelled() => return cancelled(cancel),
                    responses = async { tokio::join!(
                        Self::fetch_inventory_group(provider, pending.first().cloned()),
                        Self::fetch_inventory_group(provider, pending.get(1).cloned()),
                        Self::fetch_inventory_group(provider, pending.get(2).cloned()),
                        Self::fetch_inventory_group(provider, pending.get(3).cloned())
                    ) } => responses,
                };
                let mut error = None;
                for (index, response) in [responses.0, responses.1, responses.2, responses.3]
                    .into_iter()
                    .enumerate()
                {
                    match response {
                        Ok(Some((mut group, page))) => {
                            if page.next.as_ref().is_some_and(|next| {
                                Some(next) == group.page.as_ref() || group.seen_pages.contains(next)
                            }) {
                                let tx = self.db.unchecked_transaction()?;
                                self.clear_group_files(&group)?;
                                group.page = None;
                                group.pages = 0;
                                group.complete = false;
                                group.seen_pages.clear();
                                self.save_group(&group, false)?;
                                tx.commit()?;
                                error = Some(Error::new(
                                    ErrorCode::IncompleteScan,
                                    "Drive repeated a folder-group page. Resume retries that group and preserves the other checkpoints.",
                                ));
                                continue;
                            }
                            let tx = self.db.unchecked_transaction()?;
                            for item in page.items {
                                self.remote_item(&item)?;
                            }
                            if let Some(old) = group.page.take() {
                                group.seen_pages.insert(old);
                            }
                            group.page = page.next;
                            group.pages += 1;
                            group.complete = group.page.is_none();
                            self.save_group(&group, false)?;
                            self.inventory_metrics(progress)?;
                            tx.commit()?;
                        }
                        Ok(None) => {}
                        Err(failure) => {
                            if failure.code == ErrorCode::InvalidConfig
                                && let Some(group) =
                                    pending.get(index).filter(|group| group.page.is_some())
                            {
                                let tx = self.db.unchecked_transaction()?;
                                self.clear_group_files(group)?;
                                let mut group = group.clone();
                                group.page = None;
                                group.pages = 0;
                                group.complete = false;
                                group.seen_pages.clear();
                                self.save_group(&group, false)?;
                                tx.commit()?;
                                error = Some(Error::new(
                                    ErrorCode::IncompleteScan,
                                    "The saved folder-group page expired. Resume retries that group and preserves the other checkpoints.",
                                ));
                            } else {
                                error = Some(failure);
                            }
                        }
                    }
                }
                if let Some(error) = error {
                    return Err(error);
                }
                continue;
            }
            self.grouped_changes(provider, cancel, true).await?;
            let changes = self.inventory_deltas()?;
            let tx = self.db.unchecked_transaction()?;
            let mut exhausted = false;
            for id in &active {
                let mut group = self.group(*id)?;
                // A complete single response has no page boundary across which
                // an unchanged child can be skipped. Merge its intervening feed.
                // Multi-page groups instead require a quiet scoped window.
                let dirty =
                    group.pages > 1 && changes.iter().any(|change| change.affects(&group.parents));
                if dirty {
                    let attempts = retries.entry(*id).or_default();
                    *attempts += 1;
                    exhausted |= *attempts >= 3;
                    self.clear_group_files(&group)?;
                    group.page = None;
                    group.pages = 0;
                    group.complete = false;
                    group.seen_pages.clear();
                } else {
                    self.set(
                        "inventory_folders_checked",
                        &(self
                            .get::<u64>("inventory_folders_checked")?
                            .unwrap_or_default()
                            + group.parents.len() as u64),
                    )?;
                }
                self.save_group(&group, !dirty)?;
            }
            self.db.execute("DELETE FROM inventory_deltas", [])?;
            self.set("active_inventory_groups", &Vec::<i64>::new())?;
            self.inventory_metrics(progress)?;
            tx.commit()?;
            if exhausted {
                return Err(Error::new(
                    ErrorCode::IncompleteScan,
                    "A Drive folder group kept changing during pagination. Resume retries affected folders and keeps the other verified groups.",
                ));
            }
        }
    }
}
