//! Deterministic cloud fixtures with pagination, resumable writes and injected faults.
use crate::{
    Account, Error, ErrorCode, Fingerprint, ItemKind, RemoteItem, Result, UploadSession,
    provider::{Capabilities, Change, Changes, Page, Provider, UploadProgress, UploadRequest},
};
use async_trait::async_trait;
use md5::{Digest, Md5};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Copy)]
pub enum Fault {
    Pass,
    Permission,
    Timeout,
    RateLimit,
    UncertainSuccess,
    CursorExpired,
}
#[derive(Clone)]
pub struct FakeProvider {
    state: Arc<Mutex<State>>,
    page_size: usize,
}
#[derive(Clone)]
struct Stored {
    item: RemoteItem,
    content: Vec<u8>,
}
#[derive(Clone)]
struct Session {
    request: UploadRequest,
    bytes: Vec<u8>,
    complete: Option<RemoteItem>,
}
struct State {
    items: BTreeMap<String, Stored>,
    sessions: BTreeMap<String, Session>,
    changes: Vec<Change>,
    faults: BTreeMap<String, VecDeque<Fault>>,
    download_change: Option<(String, Vec<u8>)>,
    listing_change: Option<(usize, String, Vec<u8>)>,
    next_id: u64,
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self::new(2)
    }
}
impl FakeProvider {
    pub fn new(page_size: usize) -> Self {
        let root = item("root", "root", None, ItemKind::Folder, &[], 1, None);
        Self {
            page_size: page_size.max(1),
            state: Arc::new(Mutex::new(State {
                items: BTreeMap::from([(
                    "root".into(),
                    Stored {
                        item: root,
                        content: Vec::new(),
                    },
                )]),
                sessions: BTreeMap::new(),
                changes: Vec::new(),
                faults: BTreeMap::new(),
                download_change: None,
                listing_change: None,
                next_id: 1,
            })),
        }
    }
    pub fn inject(&self, method: &str, fault: Fault) {
        self.state
            .lock()
            .unwrap()
            .faults
            .entry(method.into())
            .or_default()
            .push_back(fault);
    }
    pub fn change_during_download(&self, id: &str, bytes: Vec<u8>) {
        self.state.lock().unwrap().download_change = Some((id.into(), bytes));
    }
    pub fn change_after_listing_calls(&self, calls: usize, id: &str, bytes: Vec<u8>) {
        self.state.lock().unwrap().listing_change = Some((calls.max(1), id.into(), bytes));
    }
    pub fn seed(&self, parent: &str, name: &str, bytes: &[u8], kind: ItemKind) -> String {
        let mut s = self.state.lock().unwrap();
        let id = format!("fake-{}", s.next_id);
        s.next_id += 1;
        let value = item(&id, name, Some(parent), kind, bytes, 1, None);
        s.items.insert(
            id.clone(),
            Stored {
                item: value.clone(),
                content: bytes.to_vec(),
            },
        );
        s.changes.push(Change {
            id: id.clone(),
            removed: false,
            item: Some(value),
        });
        id
    }
    pub fn edit(&self, id: &str, bytes: &[u8]) {
        let mut s = self.state.lock().unwrap();
        let stored = s.items.get_mut(id).unwrap();
        stored.content = bytes.to_vec();
        stored.item.fingerprint = Some(hash(bytes));
        bump(&mut stored.item);
        let updated = stored.item.clone();
        s.changes.push(Change {
            id: id.into(),
            removed: false,
            item: Some(updated),
        });
    }
    pub fn deny(&self, id: &str) {
        if let Some(i) = self.state.lock().unwrap().items.get_mut(id) {
            i.item.can_edit = false;
            i.item.can_add_children = false;
        }
    }
    fn listed(&self) {
        let change = {
            let mut state = self.state.lock().unwrap();
            if let Some((remaining, _, _)) = &mut state.listing_change {
                *remaining -= 1;
            }
            if state
                .listing_change
                .as_ref()
                .is_some_and(|(remaining, _, _)| *remaining == 0)
            {
                state.listing_change.take()
            } else {
                None
            }
        };
        if let Some((_, id, bytes)) = change {
            self.edit(&id, &bytes);
        }
    }
    fn before(&self, method: &str) -> Result<bool> {
        let fault = self
            .state
            .lock()
            .unwrap()
            .faults
            .get_mut(method)
            .and_then(|q| q.pop_front());
        match fault {
            None | Some(Fault::Pass) => Ok(false),
            Some(Fault::UncertainSuccess) => Ok(true),
            Some(Fault::Permission) => {
                Err(Error::new(ErrorCode::Permission, "Fixture access denied."))
            }
            Some(Fault::CursorExpired) => Err(Error::new(
                ErrorCode::IncompleteScan,
                "Fixture change cursor expired.",
            )),
            Some(Fault::Timeout) => Err(Error::new(ErrorCode::Transient, "Fixture timeout.")),
            Some(Fault::RateLimit) => {
                let mut e = Error::new(ErrorCode::RateLimited, "Fixture rate limit.");
                e.retry_after_secs = Some(1);
                Err(e)
            }
        }
    }
}
fn after<T>(ambiguous: bool, value: T) -> Result<T> {
    if ambiguous {
        Err(Error::new(
            ErrorCode::AmbiguousOutcome,
            "The request may have succeeded. Reconcile before retrying.",
        ))
    } else {
        Ok(value)
    }
}
fn hash(bytes: &[u8]) -> Fingerprint {
    Fingerprint {
        size: bytes.len() as u64,
        md5: format!("{:x}", Md5::digest(bytes)),
    }
}
fn bump(i: &mut RemoteItem) {
    i.version = (i.version.parse::<u64>().unwrap() + 1).to_string();
    i.etag = Some(i.version.clone());
}
fn item(
    id: &str,
    name: &str,
    parent: Option<&str>,
    kind: ItemKind,
    bytes: &[u8],
    version: u64,
    operation: Option<&str>,
) -> RemoteItem {
    RemoteItem {
        id: id.into(),
        name: name.into(),
        parents: parent.map(|p| vec![p.into()]).unwrap_or_default(),
        kind,
        fingerprint: if kind == ItemKind::File {
            Some(hash(bytes))
        } else {
            None
        },
        version: version.to_string(),
        etag: Some(version.to_string()),
        modified_time: None,
        trashed: false,
        can_download: true,
        can_edit: true,
        can_add_children: true,
        operation_id: operation.map(Into::into),
    }
}
fn check_write(s: &State, parent: &str) -> Result<()> {
    let root = s
        .items
        .get(parent)
        .ok_or_else(|| Error::new(ErrorCode::NotFound, "Fixture folder unavailable."))?;
    if root.item.trashed || root.item.kind != ItemKind::Folder {
        return Err(Error::new(
            ErrorCode::NotFound,
            "Fixture folder unavailable.",
        ));
    }
    if !root.item.can_add_children {
        return Err(Error::new(
            ErrorCode::Permission,
            "Fixture folder is read-only.",
        ));
    }
    Ok(())
}
fn check_version(actual: &RemoteItem, expected: &RemoteItem) -> Result<()> {
    if !actual.can_edit {
        return Err(Error::new(
            ErrorCode::Permission,
            "Fixture file is read-only.",
        ));
    }
    if actual.version != expected.version || actual.trashed {
        return Err(Error::new(
            ErrorCode::Conflict,
            "Fixture file changed during operation.",
        ));
    }
    Ok(())
}

#[async_trait]
impl Provider for FakeProvider {
    async fn inventory_page(&self, page: Option<&str>) -> Result<Page<RemoteItem>> {
        self.before("inventory")?;
        self.listed();
        let state = self.state.lock().unwrap();
        let offset = page
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|_| Error::new(ErrorCode::InvalidConfig, "Invalid fixture page."))?;
        let items: Vec<_> = state
            .items
            .values()
            .filter(|s| s.item.id != "root" && !s.item.trashed)
            .map(|s| s.item.clone())
            .collect();
        let end = (offset + self.page_size).min(items.len());
        let next = (end < items.len()).then(|| end.to_string());
        Ok(Page {
            items: items
                .into_iter()
                .skip(offset)
                .take(self.page_size)
                .collect(),
            next,
        })
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            md5: true,
            resumable_uploads: true,
            trash: true,
            native_documents: true,
            permissions: true,
        }
    }
    async fn identity(&self) -> Result<Account> {
        self.before("identity")?;
        Ok(Account {
            id: "fake-account".into(),
            email: "fixture@example.invalid".into(),
            display_name: "Fixture".into(),
        })
    }
    async fn get(&self, id: &str) -> Result<RemoteItem> {
        self.before("get")?;
        self.state
            .lock()
            .unwrap()
            .items
            .get(id)
            .map(|i| i.item.clone())
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "Fixture item unavailable."))
    }
    async fn children(&self, parent: &str, page: Option<&str>) -> Result<Page<RemoteItem>> {
        self.before("children")?;
        self.listed();
        let offset = page
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|_| Error::new(ErrorCode::InvalidConfig, "Invalid fixture cursor."))?;
        let s = self.state.lock().unwrap();
        let entries: Vec<_> = s
            .items
            .values()
            .filter(|i| !i.item.trashed && i.item.parents.iter().any(|p| p == parent))
            .map(|i| i.item.clone())
            .collect();
        Ok(Page {
            items: entries
                .iter()
                .skip(offset)
                .take(self.page_size)
                .cloned()
                .collect(),
            next: if offset + self.page_size < entries.len() {
                Some((offset + self.page_size).to_string())
            } else {
                None
            },
        })
    }
    async fn start_cursor(&self) -> Result<String> {
        self.before("start_cursor")?;
        Ok(self.state.lock().unwrap().changes.len().to_string())
    }
    async fn changes(&self, cursor: &str) -> Result<Changes> {
        self.before("changes")?;
        let offset = cursor
            .parse::<usize>()
            .map_err(|_| Error::new(ErrorCode::InvalidConfig, "Invalid fixture cursor."))?;
        let s = self.state.lock().unwrap();
        let next = offset + self.page_size;
        Ok(Changes {
            changes: s
                .changes
                .iter()
                .skip(offset)
                .take(self.page_size)
                .cloned()
                .collect(),
            next: if next < s.changes.len() {
                Some(next.to_string())
            } else {
                None
            },
            new_cursor: if next >= s.changes.len() {
                Some(s.changes.len().to_string())
            } else {
                None
            },
        })
    }
    async fn download(&self, id: &str, offset: u64, max_bytes: usize) -> Result<Vec<u8>> {
        self.before("download")?;
        let change = self.state.lock().unwrap().download_change.take();
        if let Some((target, bytes)) = change {
            self.edit(&target, &bytes);
        }
        let s = self.state.lock().unwrap();
        let i = s
            .items
            .get(id)
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "Fixture item unavailable."))?;
        if !i.item.can_download {
            return Err(Error::new(
                ErrorCode::Permission,
                "Fixture download denied.",
            ));
        }
        Ok(i.content
            .iter()
            .skip(offset as usize)
            .take(max_bytes)
            .copied()
            .collect())
    }
    async fn reserve_id(&self) -> Result<String> {
        let mut s = self.state.lock().unwrap();
        let id = format!("fake-{}", s.next_id);
        s.next_id += 1;
        Ok(id)
    }
    async fn create_folder(
        &self,
        parent: &str,
        name: &str,
        operation: &str,
        reserved_id: &str,
    ) -> Result<RemoteItem> {
        let ambiguous = self.before("create_folder")?;
        let mut s = self.state.lock().unwrap();
        check_write(&s, parent)?;
        if let Some(i) = s.items.values().find(|i| {
            i.item.operation_id.as_deref() == Some(operation)
                && i.item.parents.contains(&parent.into())
        }) {
            return Ok(i.item.clone());
        }
        if s.items.contains_key(reserved_id) {
            return Err(Error::new(
                ErrorCode::Conflict,
                "The reserved fixture ID already exists.",
            ));
        }
        let id = reserved_id.to_owned();
        let i = item(
            &id,
            name,
            Some(parent),
            ItemKind::Folder,
            &[],
            1,
            Some(operation),
        );
        s.items.insert(
            id.clone(),
            Stored {
                item: i.clone(),
                content: Vec::new(),
            },
        );
        s.changes.push(Change {
            id,
            removed: false,
            item: Some(i.clone()),
        });
        after(ambiguous, i)
    }
    async fn find_operation(&self, parent: &str, operation: &str) -> Result<Vec<RemoteItem>> {
        self.before("find_operation")?;
        Ok(self
            .state
            .lock()
            .unwrap()
            .items
            .values()
            .filter(|i| {
                !i.item.trashed
                    && i.item.parents.contains(&parent.into())
                    && i.item.operation_id.as_deref() == Some(operation)
            })
            .map(|i| i.item.clone())
            .collect())
    }
    async fn start_upload(&self, request: &UploadRequest) -> Result<UploadSession> {
        self.before("start_upload")?;
        let mut s = self.state.lock().unwrap();
        check_write(&s, &request.parent_id)?;
        if let Some(existing) = &request.existing {
            check_version(
                &s.items
                    .get(&existing.id)
                    .ok_or_else(|| Error::new(ErrorCode::NotFound, "Fixture file unavailable."))?
                    .item,
                existing,
            )?;
        }
        let url = format!("fake:{}", uuid::Uuid::new_v4());
        s.sessions.insert(
            url.clone(),
            Session {
                request: request.clone(),
                bytes: Vec::new(),
                complete: None,
            },
        );
        Ok(UploadSession {
            url,
            uploaded_bytes: 0,
            total_bytes: request.size,
            expected_remote: request.existing.clone(),
            target_id: request.reserved_id.clone(),
        })
    }
    async fn upload_chunk(
        &self,
        session: &UploadSession,
        bytes: Vec<u8>,
    ) -> Result<UploadProgress> {
        let ambiguous = self.before("upload_chunk")?;
        let mut s = self.state.lock().unwrap();
        let upload = s
            .sessions
            .get_mut(&session.url)
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "Fixture upload expired."))?;
        if let Some(done) = &upload.complete {
            return Ok(UploadProgress::Complete(done.clone()));
        }
        if upload.bytes.len() as u64 != session.uploaded_bytes
            || upload.bytes.len() as u64 + bytes.len() as u64 > session.total_bytes
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "Fixture upload offset mismatch.",
            ));
        }
        upload.bytes.extend(bytes);
        if (upload.bytes.len() as u64) < session.total_bytes {
            return after(
                ambiguous,
                UploadProgress::Continue(upload.bytes.len() as u64),
            );
        }
        let request = upload.request.clone();
        let content = upload.bytes.clone();
        let (id, version) = if let Some(existing) = &request.existing {
            let actual = &s.items.get(&existing.id).unwrap().item;
            check_version(actual, existing)?;
            (
                existing.id.clone(),
                actual.version.parse::<u64>().unwrap() + 1,
            )
        } else {
            let id = request.reserved_id.clone().ok_or_else(|| {
                Error::new(
                    ErrorCode::InvalidConfig,
                    "Reserve an ID before creating a file.",
                )
            })?;
            if s.items.contains_key(&id) {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "The reserved fixture ID already exists.",
                ));
            }
            (id, 1)
        };
        let i = item(
            &id,
            &request.name,
            Some(&request.parent_id),
            ItemKind::File,
            &content,
            version,
            Some(&request.operation_id),
        );
        s.items.insert(
            id.clone(),
            Stored {
                item: i.clone(),
                content,
            },
        );
        s.changes.push(Change {
            id,
            removed: false,
            item: Some(i.clone()),
        });
        s.sessions.get_mut(&session.url).unwrap().complete = Some(i.clone());
        after(ambiguous, UploadProgress::Complete(i))
    }
    async fn upload_status(&self, session: &UploadSession) -> Result<UploadProgress> {
        self.before("upload_status")?;
        let s = self.state.lock().unwrap();
        let upload = s
            .sessions
            .get(&session.url)
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "Fixture upload expired."))?;
        Ok(if let Some(i) = &upload.complete {
            UploadProgress::Complete(i.clone())
        } else {
            UploadProgress::Continue(upload.bytes.len() as u64)
        })
    }
    async fn move_item(
        &self,
        expected: &RemoteItem,
        parent: &str,
        name: &str,
    ) -> Result<RemoteItem> {
        let ambiguous = self.before("move_item")?;
        let mut s = self.state.lock().unwrap();
        check_write(&s, parent)?;
        let i = s
            .items
            .get_mut(&expected.id)
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "Fixture file unavailable."))?;
        check_version(&i.item, expected)?;
        i.item.parents = vec![parent.into()];
        i.item.name = name.into();
        bump(&mut i.item);
        let updated = i.item.clone();
        s.changes.push(Change {
            id: updated.id.clone(),
            removed: false,
            item: Some(updated.clone()),
        });
        after(ambiguous, updated)
    }
    async fn trash(&self, expected: &RemoteItem) -> Result<RemoteItem> {
        let ambiguous = self.before("trash")?;
        let mut s = self.state.lock().unwrap();
        let i = s
            .items
            .get_mut(&expected.id)
            .ok_or_else(|| Error::new(ErrorCode::NotFound, "Fixture file unavailable."))?;
        if i.item.trashed {
            return Ok(i.item.clone());
        }
        check_version(&i.item, expected)?;
        i.item.trashed = true;
        bump(&mut i.item);
        let updated = i.item.clone();
        s.changes.push(Change {
            id: updated.id.clone(),
            removed: false,
            item: Some(updated.clone()),
        });
        after(ambiguous, updated)
    }
}
