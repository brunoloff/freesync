use crate::auth::{Auth, http_client, network_error};
use async_trait::async_trait;
use freesync_core::{provider::*, *};
use serde::Deserialize;
use std::{collections::BTreeMap, sync::Arc};

const BASE: &str = "https://www.googleapis.com/drive/v3";
pub const FILE_FIELDS: &str = "id,name,parents,mimeType,md5Checksum,size,version,modifiedTime,trashed,capabilities(canDownload,canEdit,canAddChildren),appProperties";

pub struct GoogleDrive {
    pub auth: Arc<Auth>,
    client: reqwest::Client,
    pub page_size: u32,
    write_root: Option<String>,
    write_parent: Option<String>,
}
impl GoogleDrive {
    pub async fn saved(account: &str) -> Result<Self> {
        let provider = Self {
            auth: Arc::new(Auth::saved(account).await?),
            client: http_client()?,
            page_size: 1000,
            write_root: None,
            write_parent: None,
        };
        let identity = provider.identity().await?;
        if !identity.email.eq_ignore_ascii_case(account) {
            return Err(Error::new(
                ErrorCode::Authentication,
                "Saved credentials belong to a different Google account. Reconnect.",
            ));
        }
        Ok(provider)
    }
    /// A saved private mapping, rather than a caller-supplied boolean, authorizes test writes.
    pub async fn for_test_pair(pair: &PairConfig) -> Result<Self> {
        let mapping: serde_json::Value = serde_json::from_slice(&std::fs::read(
            crate::auth::config_directory().join("development.json"),
        )?)?;
        let configured: PairConfig = serde_json::from_value(mapping["test_pair"].clone())?;
        if !pair.test_only
            || pair.remote_root_id != configured.remote_root_id
            || !pair
                .account_email
                .eq_ignore_ascii_case(&configured.account_email)
            || freesync_core::local::canonical_root(&pair.local_root)?
                != freesync_core::local::canonical_root(&configured.local_root)?
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "Live writes are restricted to the authorized test-freesync mapping.",
            ));
        }
        let mut drive = Self::saved(&pair.account_email).await?;
        drive.write_root = Some(pair.remote_root_id.clone());
        drive.write_parent = Some(
            mapping["read_only_adoption"]["remote_root_id"]
                .as_str()
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::InvalidConfig,
                        "The test root's parent mapping is missing.",
                    )
                })?
                .into(),
        );
        drive.healthy_write_root().await?;
        Ok(drive)
    }
    async fn healthy_write_root(&self) -> Result<&str> {
        let root = self.write_root.as_deref().ok_or_else(|| {
            Error::new(
                ErrorCode::Permission,
                "Configure the authorized test pair before performing transfers.",
            )
        })?;
        let item = self.get(root).await?;
        if item.trashed
            || item.kind != ItemKind::Folder
            || item.name != "test-freesync"
            || !item
                .parents
                .iter()
                .any(|p| Some(p) == self.write_parent.as_ref())
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "The authorized test root changed. Transfers are stopped.",
            ));
        }
        Ok(root)
    }
    async fn writable_parent(&self, parent: &str) -> Result<()> {
        let root = self.healthy_write_root().await?;
        let item = self.get(parent).await?;
        if item.trashed
            || item.kind != ItemKind::Folder
            || !item.can_add_children
            || freesync_core::remote::ancestry(self, parent, root).await?
                != freesync_core::remote::Ancestry::Inside
        {
            return Err(Error::new(
                ErrorCode::Permission,
                "The destination folder is unavailable or outside the test root.",
            ));
        }
        Ok(())
    }
    async fn writable_item(&self, expected: &RemoteItem) -> Result<RemoteItem> {
        let root = self.healthy_write_root().await?;
        if expected.id == root
            || freesync_core::remote::ancestry(self, &expected.id, root).await?
                != freesync_core::remote::Ancestry::Inside
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "This item is outside the writable test folder.",
            ));
        }
        let actual = self.get(&expected.id).await?;
        if actual.trashed
            || actual.version != expected.version
            || !actual.content_eq(expected)
            || actual.parents != expected.parents
            || actual.name != expected.name
        {
            return Err(Error::new(
                ErrorCode::Conflict,
                "Drive content or its location changed. Both versions are preserved.",
            ));
        }
        if !actual.can_edit {
            return Err(Error::new(
                ErrorCode::Permission,
                "Drive does not allow editing this item.",
            ));
        }
        Ok(actual)
    }
    async fn upload_result(
        &self,
        response: reqwest::Response,
        session: &UploadSession,
    ) -> Result<UploadProgress> {
        if response.status().as_u16() == 308 {
            let offset = response
                .headers()
                .get("range")
                .and_then(|s| s.to_str().ok())
                .map(|s| {
                    s.strip_prefix("bytes=0-")
                        .and_then(|n| n.parse::<u64>().ok())
                        .and_then(|n| n.checked_add(1))
                        .ok_or_else(|| {
                            Error::new(
                                ErrorCode::Conflict,
                                "Google returned an invalid upload offset.",
                            )
                        })
                })
                .transpose()?
                .unwrap_or(0);
            if offset > session.total_bytes {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "Google upload offset exceeds the source length.",
                ));
            }
            return Ok(UploadProgress::Continue(offset));
        }
        let result: serde_json::Value = response.json().await.map_err(network_error)?;
        let id = result["id"].as_str().ok_or_else(|| {
            Error::new(
                ErrorCode::AmbiguousOutcome,
                "Google upload completed without an item identity. Refresh before retrying.",
            )
        })?;
        if session.target_id.as_deref() != Some(id) {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "Google returned an unexpected upload identity.",
            ));
        }
        Ok(UploadProgress::Complete(self.get(id).await?))
    }
    /// Integration probe: the server must reject a stale tag on a disposable item.
    pub async fn verify_conditional_write(&self, id: &str) -> Result<()> {
        let actual = self.get(id).await?;
        self.writable_item(&actual).await?;
        if actual.kind != ItemKind::Folder
            || !actual.name.starts_with("run-")
            || actual.operation_id.is_none()
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "The conditional-write probe requires a disposable run folder.",
            ));
        }
        let tag = actual.etag.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::Unsupported,
                "The probe item has no conditional-write tag.",
            )
        })?;
        let probe_name = format!("{}-conditional-check", actual.name);
        self.send(
            self.client
                .patch(format!("https://www.googleapis.com/drive/v2/files/{id}"))
                .header("If-Match", tag)
                .json(&serde_json::json!({"title":probe_name})),
        )
        .await?;
        let updated = self.get(id).await?;
        if updated.etag == actual.etag {
            return Err(Error::new(
                ErrorCode::Unsupported,
                "The probe did not advance Google's concurrency tag.",
            ));
        }
        // Use a real, formerly valid tag. Arbitrary malformed tags can produce
        // Google backend errors rather than a meaningful precondition response.
        let outcome = self
            .send(
                self.client
                    .patch(format!("https://www.googleapis.com/drive/v2/files/{id}"))
                    .header("If-Match", tag)
                    .json(&serde_json::json!({"title":actual.name})),
            )
            .await;
        let current = self.get(id).await?;
        if current.name == probe_name {
            self.send(
                self.client
                    .patch(format!("https://www.googleapis.com/drive/v2/files/{id}"))
                    .header(
                        "If-Match",
                        current.etag.ok_or_else(|| {
                            Error::new(ErrorCode::Unsupported, "The probe restore tag is missing.")
                        })?,
                    )
                    .json(&serde_json::json!({"title":actual.name})),
            )
            .await?;
        }
        match outcome {
            Err(e) if e.code == ErrorCode::Conflict => Ok(()),
            Err(e) => Err(e),
            Ok(_) => Err(Error::new(
                ErrorCode::Unsupported,
                "Google did not enforce the required conditional-write guard.",
            )),
        }
    }
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        let token = self.auth.bearer(false).await?;
        let retry = request.try_clone();
        let response = request
            .bearer_auth(token)
            .send()
            .await
            .map_err(network_error)?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            let token = self.auth.bearer(true).await?;
            let retry = retry.ok_or_else(|| {
                Error::new(
                    ErrorCode::Authentication,
                    "Authorization changed. Retry the pending operation.",
                )
            })?;
            return check_response(
                retry
                    .bearer_auth(token)
                    .send()
                    .await
                    .map_err(network_error)?,
            )
            .await;
        }
        check_response(response).await
    }
    async fn list(&self, query: &str, page: Option<&str>) -> Result<Page<RemoteItem>> {
        let mut request = self.client.get(format!("{BASE}/files")).query(&[
            ("q", query),
            (
                "fields",
                &format!("nextPageToken,incompleteSearch,files({FILE_FIELDS})"),
            ),
            ("pageSize", &self.page_size.clamp(1, 1000).to_string()),
            ("spaces", "drive"),
        ]);
        if let Some(page) = page {
            request = request.query(&[("pageToken", page)]);
        }
        let result: FileList = self
            .send(request)
            .await?
            .json()
            .await
            .map_err(network_error)?;
        if result.incomplete_search {
            return Err(Error::new(
                ErrorCode::IncompleteScan,
                "Google returned an incomplete folder listing. No deletions will be inferred.",
            ));
        }
        Ok(Page {
            items: result
                .files
                .into_iter()
                .map(|f| f.into_item(None))
                .collect::<Result<_>>()?,
            next: result.next_page_token,
        })
    }
    pub async fn find_named_folders(&self, parent: &str, name: &str) -> Result<Vec<RemoteItem>> {
        let query = format!(
            "'{}' in parents and name = '{}' and mimeType = 'application/vnd.google-apps.folder' and trashed = false",
            escape(parent),
            escape(name)
        );
        let mut result = vec![];
        let mut cursor = None;
        loop {
            let page = self.list(&query, cursor.as_deref()).await?;
            result.extend(page.items);
            cursor = page.next;
            if cursor.is_none() {
                break;
            }
        }
        Ok(result)
    }
    /// The only provisioning write permitted on an otherwise read-only connection.
    /// The target is fixed to a named disposable folder directly beneath My Drive.
    pub async fn provision_test_root(&self) -> Result<RemoteItem> {
        let parent = self.get("root").await?;
        if !parent.can_add_children {
            return Err(Error::new(
                ErrorCode::Permission,
                "My Drive does not allow creating the test folder.",
            ));
        }
        let matches = self.find_named_folders(&parent.id, "test-freesync").await?;
        match matches.len() {
            1 => return Ok(matches[0].clone()),
            0 => (),
            _ => {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "Multiple test-freesync folders exist. Select an unambiguous folder ID.",
                ));
            }
        }
        let operation = "freesync-provision-test-root-v1";
        // A request timeout may conceal successful creation. Search before retrying.
        for attempt in 0..3 {
            let prior = self.find_operation(&parent.id, operation).await?;
            if prior.len() == 1 {
                return Ok(prior[0].clone());
            }
            if prior.len() > 1 {
                return Err(Error::new(
                    ErrorCode::Conflict,
                    "Test-folder provisioning produced ambiguous results.",
                ));
            }
            let request = self.client.post(format!("{BASE}/files")).query(&[("fields", FILE_FIELDS)])
                .json(&serde_json::json!({"name":"test-freesync","mimeType":"application/vnd.google-apps.folder","parents":[parent.id],"appProperties":{"freesyncOperation":operation}}));
            match self.send(request).await {
                Ok(response) => {
                    return response
                        .json::<DriveFile>()
                        .await
                        .map_err(network_error)?
                        .into_item(None);
                }
                Err(e) if e.retryable() && attempt < 2 => {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await
                }
                Err(e) => return Err(e),
            }
        }
        Err(Error::new(
            ErrorCode::AmbiguousOutcome,
            "Test folder creation remains uncertain. Refresh before retrying.",
        ))
    }

    /// Drive v3 omits the File ETag field. v2 still exposes the entity's conditional-write tag.
    /// Version matching prevents combining a stale v3 snapshot with a newer tag.
    async fn concurrency_tag(&self, item: &RemoteItem) -> Result<String> {
        let response = self
            .send(
                self.client
                    .get(format!(
                        "https://www.googleapis.com/drive/v2/files/{}",
                        item.id
                    ))
                    .query(&[("fields", "id,etag,version")]),
            )
            .await?;
        let value: serde_json::Value = response.json().await.map_err(network_error)?;
        if value["version"].as_str() != Some(item.version.as_str()) {
            return Err(Error::new(
                ErrorCode::Transient,
                "The Drive item changed while its concurrency tag was read. Retry.",
            ));
        }
        value["etag"].as_str().map(str::to_owned).ok_or_else(|| Error::new(ErrorCode::Unsupported,
            "Google did not provide a conditional-write tag. This item cannot be changed safely."))
    }
}
pub async fn check_response(response: reqwest::Response) -> Result<reqwest::Response> {
    let status = response.status();
    if status.is_success() || status.as_u16() == 308 {
        return Ok(response);
    }
    let retry = response
        .headers()
        .get("retry-after")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse().ok());
    let body: serde_json::Value = response.json().await.unwrap_or_default();
    if status.as_u16() == 400 {
        let source = body["error"]["message"]
            .as_str()
            .unwrap_or("")
            .to_lowercase();
        let message = if source.contains("content-range") {
            "Google rejected the upload range."
        } else if source.contains("content-length") || source.contains("size") {
            "Google rejected the upload length."
        } else if source.contains("pre-generated")
            || source.contains("file id")
            || source.contains("generated id")
        {
            "Google rejected the reserved file identity."
        } else {
            "Google rejected a request parameter."
        };
        return Err(Error::new(ErrorCode::InvalidConfig, message));
    }
    let rate = body["error"]["errors"].as_array().is_some_and(|items| {
        items.iter().any(|i| {
            matches!(
                i["reason"].as_str(),
                Some("rateLimitExceeded" | "userRateLimitExceeded")
            )
        })
    });
    let (code, message) = match status.as_u16() {
        401 => (
            ErrorCode::Authentication,
            "Google authorization is unavailable. Reconnect the account.",
        ),
        403 if rate => (
            ErrorCode::RateLimited,
            "Google is limiting requests. FreeSync will retry.",
        ),
        403 => (
            ErrorCode::Permission,
            "Google denied this operation. Check the file or folder permissions.",
        ),
        404 => (
            ErrorCode::NotFound,
            "The Drive item is unavailable. It may have been removed or access may have changed.",
        ),
        409 | 412 => (
            ErrorCode::Conflict,
            "The Drive item changed before the operation completed.",
        ),
        429 => (
            ErrorCode::RateLimited,
            "Google is limiting requests. FreeSync will retry.",
        ),
        410 => (
            ErrorCode::IncompleteScan,
            "The Drive change cursor expired. A fresh inventory is required.",
        ),
        500..=599 => (
            ErrorCode::Transient,
            "Google is temporarily unavailable. Pending work will be retried.",
        ),
        _ => (
            ErrorCode::InvalidConfig,
            "Google rejected the request. Check the selected item and application configuration.",
        ),
    };
    let mut error = Error::new(code, message);
    error.retry_after_secs = retry;
    Err(error)
}
pub fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct FileList {
    #[serde(default)]
    files: Vec<DriveFile>,
    next_page_token: Option<String>,
    #[serde(default)]
    incomplete_search: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriveFile {
    id: String,
    name: String,
    #[serde(default)]
    parents: Vec<String>,
    mime_type: String,
    md5_checksum: Option<String>,
    size: Option<String>,
    version: Option<String>,
    modified_time: Option<String>,
    #[serde(default)]
    trashed: bool,
    #[serde(default)]
    capabilities: FileCapabilities,
    #[serde(default)]
    app_properties: BTreeMap<String, String>,
}
#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct FileCapabilities {
    #[serde(default)]
    can_download: bool,
    #[serde(default)]
    can_edit: bool,
    #[serde(default)]
    can_add_children: bool,
}
impl DriveFile {
    pub fn into_item(self, etag: Option<String>) -> Result<RemoteItem> {
        let kind = match self.mime_type.as_str() {
            "application/vnd.google-apps.folder" => ItemKind::Folder,
            "application/vnd.google-apps.shortcut" => ItemKind::Shortcut,
            mime if mime.starts_with("application/vnd.google-apps.") => ItemKind::NativeDocument,
            _ => ItemKind::File,
        };
        let fingerprint = match (self.md5_checksum, self.size) {
            (Some(md5), Some(size)) => Some(Fingerprint {
                md5,
                size: size.parse().map_err(|_| {
                    Error::new(
                        ErrorCode::IncompleteScan,
                        "Google returned invalid file metadata.",
                    )
                })?,
            }),
            _ => None,
        };
        Ok(RemoteItem {
            id: self.id,
            name: self.name,
            parents: self.parents,
            kind,
            fingerprint,
            version: self.version.unwrap_or_default(),
            modified_time: self.modified_time,
            etag,
            trashed: self.trashed,
            can_download: self.capabilities.can_download,
            can_edit: self.capabilities.can_edit,
            can_add_children: self.capabilities.can_add_children,
            operation_id: self.app_properties.get("freesyncOperation").cloned(),
        })
    }
}
fn session_url(value: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(value)
        .map_err(|_| Error::new(ErrorCode::UnsafePath, "Invalid Google upload session."))?;
    if url.scheme() != "https"
        || url.host_str() != Some("www.googleapis.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !(url.path().starts_with("/upload/drive/v3/files")
            || url.path().starts_with("/upload/drive/v2/files"))
        || !url
            .query_pairs()
            .any(|(k, v)| k == "upload_id" && !v.is_empty())
    {
        return Err(Error::new(
            ErrorCode::UnsafePath,
            "Google upload session has an unexpected destination.",
        ));
    }
    Ok(url)
}

#[async_trait]
impl Provider for GoogleDrive {
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
        let response = self
            .send(
                self.client
                    .get(format!("{BASE}/about"))
                    .query(&[("fields", "user(emailAddress,displayName,permissionId)")]),
            )
            .await?;
        let value: serde_json::Value = response.json().await.map_err(network_error)?;
        let user = &value["user"];
        let field = |key: &str| {
            user[key].as_str().map(str::to_owned).ok_or_else(|| {
                Error::new(
                    ErrorCode::Authentication,
                    "Google account identity is incomplete.",
                )
            })
        };
        Ok(Account {
            id: field("permissionId")?,
            email: field("emailAddress")?,
            display_name: field("displayName")?,
        })
    }
    async fn get(&self, id: &str) -> Result<RemoteItem> {
        for attempt in 0..5 {
            let response = self
                .send(
                    self.client
                        .get(format!("{BASE}/files/{id}"))
                        .query(&[("fields", FILE_FIELDS)]),
                )
                .await?;
            let etag = response
                .headers()
                .get("etag")
                .and_then(|h| h.to_str().ok())
                .map(str::to_owned);
            let mut item = response
                .json::<DriveFile>()
                .await
                .map_err(network_error)?
                .into_item(etag)?;
            if item.etag.is_none() {
                match self.concurrency_tag(&item).await {
                    Ok(tag) => item.etag = Some(tag),
                    Err(e) if e.code == ErrorCode::Transient && attempt < 4 => {
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        continue;
                    }
                    Err(e) => return Err(e),
                }
            }
            return Ok(item);
        }
        Err(Error::new(
            ErrorCode::Transient,
            "Drive metadata is still changing. Retry after it settles.",
        ))
    }
    async fn children(&self, parent: &str, page: Option<&str>) -> Result<Page<RemoteItem>> {
        self.list(
            &format!("'{}' in parents and trashed = false", escape(parent)),
            page,
        )
        .await
    }
    async fn start_cursor(&self) -> Result<String> {
        let value: serde_json::Value = self
            .send(self.client.get(format!("{BASE}/changes/startPageToken")))
            .await?
            .json()
            .await
            .map_err(network_error)?;
        value["startPageToken"]
            .as_str()
            .map(Into::into)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::IncompleteScan,
                    "Google did not return a change cursor.",
                )
            })
    }
    async fn changes(&self, cursor: &str) -> Result<Changes> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct ChangePage {
            #[serde(default)]
            changes: Vec<DriveChange>,
            next_page_token: Option<String>,
            new_start_page_token: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct DriveChange {
            file_id: String,
            #[serde(default)]
            removed: bool,
            file: Option<DriveFile>,
        }
        let fields =
            format!("nextPageToken,newStartPageToken,changes(fileId,removed,file({FILE_FIELDS}))");
        let response: ChangePage = self
            .send(self.client.get(format!("{BASE}/changes")).query(&[
                ("pageToken", cursor),
                ("fields", &fields),
                ("pageSize", &self.page_size.to_string()),
                ("includeRemoved", "true"),
                ("spaces", "drive"),
            ]))
            .await?
            .json()
            .await
            .map_err(network_error)?;
        Ok(Changes {
            changes: response
                .changes
                .into_iter()
                .map(|c| {
                    Ok(Change {
                        id: c.file_id,
                        removed: c.removed,
                        item: c.file.map(|f| f.into_item(None)).transpose()?,
                    })
                })
                .collect::<Result<_>>()?,
            next: response.next_page_token,
            new_cursor: response.new_start_page_token,
        })
    }
    async fn download(&self, id: &str, offset: u64, max_bytes: usize) -> Result<Vec<u8>> {
        if max_bytes == 0 {
            return Ok(vec![]);
        }
        let response = self
            .send(
                self.client
                    .get(format!("{BASE}/files/{id}"))
                    .query(&[("alt", "media")])
                    .header(
                        "Range",
                        format!(
                            "bytes={offset}-{}",
                            offset.saturating_add(max_bytes as u64).saturating_sub(1)
                        ),
                    ),
            )
            .await?;
        if offset > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            return Err(Error::new(
                ErrorCode::Conflict,
                "Google did not honor the download resume offset.",
            ));
        }
        if let Some(len) = response.content_length()
            && len > max_bytes as u64
        {
            return Err(Error::new(
                ErrorCode::Unsupported,
                "Google did not honor the requested download range.",
            ));
        }
        let bytes = response.bytes().await.map_err(network_error)?;
        if bytes.len() > max_bytes {
            return Err(Error::new(
                ErrorCode::Unsupported,
                "Google returned an oversized download chunk.",
            ));
        }
        Ok(bytes.to_vec())
    }
    async fn reserve_id(&self) -> Result<String> {
        self.healthy_write_root().await?;
        let value: serde_json::Value = self
            .send(
                self.client
                    .get(format!("{BASE}/files/generateIds"))
                    .query(&[("count", "1"), ("space", "drive"), ("type", "files")]),
            )
            .await?
            .json()
            .await
            .map_err(network_error)?;
        value["ids"][0].as_str().map(str::to_owned).ok_or_else(|| {
            Error::new(
                ErrorCode::Transient,
                "Google did not reserve a new file identity.",
            )
        })
    }
    async fn create_folder(
        &self,
        parent: &str,
        name: &str,
        operation: &str,
        reserved_id: &str,
    ) -> Result<RemoteItem> {
        self.writable_parent(parent).await?;
        freesync_core::local::validate_relative(name)?;
        let response=self.send(self.client.post(format!("{BASE}/files")).query(&[("fields",FILE_FIELDS)]).json(&serde_json::json!({"id":reserved_id,"name":name,"parents":[parent],"mimeType":"application/vnd.google-apps.folder","appProperties":{"freesyncOperation":operation}}))).await?;
        response
            .json::<DriveFile>()
            .await
            .map_err(network_error)?
            .into_item(None)
    }
    async fn find_operation(&self, parent: &str, operation: &str) -> Result<Vec<RemoteItem>> {
        let query = format!(
            "'{}' in parents and trashed = false and appProperties has {{ key='freesyncOperation' and value='{}' }}",
            escape(parent),
            escape(operation)
        );
        let mut items = vec![];
        let mut page = None;
        loop {
            let result = self.list(&query, page.as_deref()).await?;
            items.extend(result.items);
            page = result.next;
            if page.is_none() {
                break;
            }
        }
        Ok(items)
    }
    async fn start_upload(&self, request: &UploadRequest) -> Result<UploadSession> {
        self.writable_parent(&request.parent_id).await?;
        freesync_core::local::validate_relative(&request.name)?;
        let (builder, expected, target_id) = if let Some(expected) = &request.existing {
            let actual = self.writable_item(expected).await?;
            let tag = actual.etag.as_ref().ok_or_else(|| {
                Error::new(
                    ErrorCode::Unsupported,
                    "A Drive write requires a concurrency tag.",
                )
            })?;
            let builder=self.client.put(format!("https://www.googleapis.com/upload/drive/v2/files/{}",actual.id)).header("If-Match",tag).query(&[("uploadType","resumable"),("fields","id")]).json(&serde_json::json!({"properties":[{"key":"freesyncOperation","value":request.operation_id,"visibility":"PRIVATE"}]}));
            let id = actual.id.clone();
            (builder, Some(actual), id)
        } else {
            let id = request.reserved_id.clone().ok_or_else(|| {
                Error::new(
                    ErrorCode::InvalidConfig,
                    "Reserve and journal a file ID before creating an upload.",
                )
            })?;
            (self.client.post("https://www.googleapis.com/upload/drive/v3/files").query(&[("uploadType","resumable"),("fields","id")]).json(&serde_json::json!({"id":id,"name":request.name,"parents":[request.parent_id],"mimeType":"application/octet-stream","appProperties":{"freesyncOperation":request.operation_id}})),None,id)
        };
        let response = self
            .send(
                builder
                    .header("X-Upload-Content-Type", "application/octet-stream")
                    .header("X-Upload-Content-Length", request.size),
            )
            .await?;
        let url = response
            .headers()
            .get("location")
            .and_then(|s| s.to_str().ok())
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::AmbiguousOutcome,
                    "Google did not return an upload session.",
                )
            })?
            .to_owned();
        session_url(&url)?;
        Ok(UploadSession {
            url,
            uploaded_bytes: 0,
            total_bytes: request.size,
            expected_remote: expected,
            target_id: Some(target_id),
        })
    }
    async fn upload_chunk(
        &self,
        session: &UploadSession,
        bytes: Vec<u8>,
    ) -> Result<UploadProgress> {
        self.healthy_write_root().await?;
        let mut request = self
            .client
            .put(session_url(&session.url)?)
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", bytes.len());
        if let Some(expected) = &session.expected_remote {
            let actual = self.writable_item(expected).await?;
            request = request.header(
                "If-Match",
                actual.etag.ok_or_else(|| {
                    Error::new(
                        ErrorCode::Unsupported,
                        "A conditional write tag is missing.",
                    )
                })?,
            );
        }
        let end = session
            .uploaded_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| Error::new(ErrorCode::Conflict, "Upload size overflow."))?;
        if end > session.total_bytes || session.total_bytes > 0 && bytes.is_empty() {
            return Err(Error::new(
                ErrorCode::Conflict,
                "Upload chunk exceeds its source length.",
            ));
        }
        let range = if session.total_bytes == 0 {
            "bytes */0".into()
        } else {
            format!(
                "bytes {}-{}/{}",
                session.uploaded_bytes,
                end - 1,
                session.total_bytes
            )
        };
        let response = self
            .send(request.header("Content-Range", range).body(bytes))
            .await?;
        self.upload_result(response, session).await
    }
    async fn upload_status(&self, session: &UploadSession) -> Result<UploadProgress> {
        self.healthy_write_root().await?;
        let response = self
            .send(
                self.client
                    .put(session_url(&session.url)?)
                    .header("Content-Length", 0)
                    .header("Content-Range", format!("bytes */{}", session.total_bytes))
                    .body(Vec::<u8>::new()),
            )
            .await?;
        self.upload_result(response, session).await
    }
    async fn move_item(
        &self,
        expected: &RemoteItem,
        parent: &str,
        name: &str,
    ) -> Result<RemoteItem> {
        self.writable_parent(parent).await?;
        freesync_core::local::validate_relative(name)?;
        let actual = self.writable_item(expected).await?;
        let mut request = self
            .client
            .patch(format!(
                "https://www.googleapis.com/drive/v2/files/{}",
                actual.id
            ))
            .query(&[("fields", "id")])
            .header(
                "If-Match",
                actual.etag.ok_or_else(|| {
                    Error::new(
                        ErrorCode::Unsupported,
                        "A conditional write tag is missing.",
                    )
                })?,
            )
            .json(&serde_json::json!({"title":name}));
        if actual.parents != [parent] {
            request = request.query(&[
                ("addParents", parent),
                ("removeParents", &actual.parents.join(",")),
            ]);
        }
        self.send(request).await?;
        self.get(&actual.id).await
    }
    async fn trash(&self, expected: &RemoteItem) -> Result<RemoteItem> {
        let actual = self.writable_item(expected).await?;
        self.send(
            self.client
                .patch(format!(
                    "https://www.googleapis.com/drive/v2/files/{}",
                    actual.id
                ))
                .header(
                    "If-Match",
                    actual.etag.ok_or_else(|| {
                        Error::new(
                            ErrorCode::Unsupported,
                            "A conditional write tag is missing.",
                        )
                    })?,
                )
                .query(&[("fields", "id")])
                .json(&serde_json::json!({"labels":{"trashed":true}})),
        )
        .await?;
        self.get(&actual.id).await
    }
}
