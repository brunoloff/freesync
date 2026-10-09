use crate::{Account, RemoteItem, Result, UploadSession};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    pub md5: bool,
    pub resumable_uploads: bool,
    pub trash: bool,
    pub native_documents: bool,
    pub permissions: bool,
}
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}
#[derive(Debug, Clone)]
pub struct Change {
    pub id: String,
    pub removed: bool,
    pub item: Option<RemoteItem>,
}
#[derive(Debug, Clone)]
pub struct Changes {
    pub changes: Vec<Change>,
    pub next: Option<String>,
    pub new_cursor: Option<String>,
}
#[derive(Debug, Clone)]
pub struct UploadRequest {
    pub operation_id: String,
    pub parent_id: String,
    pub name: String,
    pub existing: Option<RemoteItem>,
    pub size: u64,
    pub reserved_id: Option<String>,
}
#[derive(Debug, Clone)]
pub enum UploadProgress {
    Continue(u64),
    Complete(RemoteItem),
}

/// Opaque provider IDs and cursors never masquerade as local paths.
#[async_trait]
pub trait Provider: Send + Sync {
    fn capabilities(&self) -> Capabilities;
    async fn identity(&self) -> Result<Account>;
    async fn get(&self, id: &str) -> Result<RemoteItem>;
    async fn children(&self, parent: &str, page: Option<&str>) -> Result<Page<RemoteItem>>;
    /// Read-only account inventory, used to adopt large pre-existing trees.
    /// Callers resolve ancestry from opaque IDs and consume changes after listing.
    async fn inventory_page(&self, _page: Option<&str>) -> Result<Page<RemoteItem>> {
        Err(crate::Error::new(
            crate::ErrorCode::Unsupported,
            "This provider cannot inventory an account for adoption.",
        ))
    }
    async fn start_cursor(&self) -> Result<String>;
    async fn changes(&self, cursor: &str) -> Result<Changes>;
    async fn download(&self, id: &str, offset: u64, max_bytes: usize) -> Result<Vec<u8>>;
    async fn reserve_id(&self) -> Result<String>;
    async fn create_folder(
        &self,
        parent: &str,
        name: &str,
        operation: &str,
        reserved_id: &str,
    ) -> Result<RemoteItem>;
    async fn find_operation(&self, parent: &str, operation: &str) -> Result<Vec<RemoteItem>>;
    async fn start_upload(&self, request: &UploadRequest) -> Result<UploadSession>;
    async fn upload_chunk(&self, session: &UploadSession, bytes: Vec<u8>)
    -> Result<UploadProgress>;
    async fn upload_status(&self, session: &UploadSession) -> Result<UploadProgress>;
    async fn move_item(&self, item: &RemoteItem, parent: &str, name: &str) -> Result<RemoteItem>;
    async fn trash(&self, item: &RemoteItem) -> Result<RemoteItem>;
}
