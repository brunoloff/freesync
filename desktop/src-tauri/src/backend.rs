use freesync_core::{
    db::Database,
    engine::{self, EngineCommand, EngineRequest},
    local,
    profile::{ProfileLock, default_profile},
    *,
};
use freesync_google::{
    GoogleDrive,
    auth::{PendingLogin, bootstrap_account, config_directory},
    store::NativeStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    thread::JoinHandle,
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[derive(Default, Clone, Serialize, Deserialize)]
pub struct Preferences {
    pub notifications: bool,
}
pub struct AppState {
    pub profile: PathBuf,
    pub requests: mpsc::Sender<EngineRequest>,
    pub cancel: CancellationToken,
    pub worker: Mutex<Option<JoinHandle<Result<()>>>>,
    pub login: tokio::sync::Mutex<Option<PendingLogin>>,
    pub login_cancel: Mutex<Option<CancellationToken>>,
    pub last_error: Arc<Mutex<Option<Error>>>,
}
fn internal() -> Error {
    Error::new(
        ErrorCode::Internal,
        "The background engine could not be contacted. Restart FreeSync.",
    )
}
impl AppState {
    pub fn start() -> Result<Self> {
        let profile = std::env::var_os("FREESYNC_PROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(default_profile);
        let (requests, receiver) = mpsc::channel(16);
        let (started, ready) = std::sync::mpsc::sync_channel(1);
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let directory = profile.clone();
        let last_error = Arc::new(Mutex::new(None));
        let error = last_error.clone();
        let worker = std::thread::Builder::new()
            .name("freesync-owner".into())
            .spawn(move || {
                let lock = ProfileLock::acquire(&directory);
                let owner = match lock {
                    Ok(owner) => owner,
                    Err(e) => {
                        let _ = started.send(Err(e.clone()));
                        return Err(e);
                    }
                };
                let result = (|| -> Result<()> {
                    let mut db = Database::open(&directory)?;
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()?;
                    let _ = started.send(Ok(()));
                    runtime.block_on(engine::run_controlled(
                        &mut db,
                        &directory,
                        &freesync_google::GoogleFactory,
                        token,
                        receiver,
                    ))
                })();
                if let Err(e) = &result
                    && let Ok(mut state) = error.lock()
                {
                    *state = Some(e.clone());
                }
                drop(owner);
                result
            })?;
        ready.recv().map_err(|_| internal())??;
        Ok(Self {
            profile,
            requests,
            cancel,
            worker: Mutex::new(Some(worker)),
            login: tokio::sync::Mutex::new(None),
            login_cancel: Mutex::new(None),
            last_error,
        })
    }
    pub fn database(&self) -> Result<Database> {
        Database::open(&self.profile)
    }
    pub async fn request(&self, command: EngineCommand) -> Result<Value> {
        let (reply, response) = oneshot::channel();
        self.requests
            .send(EngineRequest { command, reply })
            .await
            .map_err(|_| internal())?;
        response.await.map_err(|_| internal())?
    }
    pub fn stop(&self) -> Result<()> {
        self.cancel.cancel();
        if let Some(cancellation) = self.login_cancel.lock().map_err(|_| internal())?.take() {
            cancellation.cancel();
        }
        let worker = self.worker.lock().map_err(|_| internal())?.take();
        if let Some(worker) = worker {
            worker.join().map_err(|_| internal())??;
        }
        Ok(())
    }
    pub fn stopped(&self) -> bool {
        self.worker
            .lock()
            .ok()
            .is_none_or(|w| w.as_ref().is_none_or(JoinHandle::is_finished))
    }
    pub fn snapshot(&self) -> Result<Value> {
        let db = self.database()?;
        let pairs = db.pairs()?;
        let mut pair_views = vec![];
        let mut conflicts = vec![];
        for pair in pairs {
            let status = db.status(&pair.id)?.unwrap_or_else(|| PairStatus {
                pair_id: pair.id.clone(),
                state: "disabled".into(),
                ..Default::default()
            });
            let deletion_hold = db
                .get::<bool>(&format!("deletion_hold:{}", pair.id))?
                .unwrap_or(false);
            let deletion_count = db
                .operations(&pair.id)?
                .iter()
                .filter(|o| matches!(o.action, Action::TrashRemote | Action::RecycleLocal))
                .count();
            pair_views.push(json!({"id":pair.id,"account":pair.account_email,"local_root":pair.local_root,"remote_id":pair.remote_root_id,"remote_name":pair.remote_root_name,"enabled":pair.enabled,"poll_secs":pair.poll_secs,"deletion_limit":pair.deletion_limit,"status":status,"deletion_hold":deletion_hold,"deletion_count":deletion_count}));
            for conflict in db.conflicts(&pair.id)? {
                conflicts.push(json!({"pair_id":pair.id,"path":conflict.path,"reason":conflict.reason,"can_keep_both":conflict.local.as_ref().is_some_and(|l|l.kind==ItemKind::File) && conflict.remote.as_ref().is_some_and(|r|r.kind==ItemKind::File),"local_bytes":conflict.local.and_then(|l|l.fingerprint).map(|f|f.size),"remote_bytes":conflict.remote.and_then(|r|r.fingerprint).map(|f|f.size)}));
            }
        }
        let account = db
            .get::<String>("selected_account")?
            .or_else(|| bootstrap_account().ok());
        Ok(
            json!({"account":account,"account_verified":db.get::<bool>("account_verified")?.unwrap_or(false),"pairs":pair_views,"conflicts":conflicts,"controls":db.controls()?,"preferences":db.get::<Preferences>("preferences")?.unwrap_or_default(),"engine_error":self.last_error.lock().map_err(|_|internal())?.clone(),"recovery_directory":self.profile.join("recovery")}),
        )
    }
    pub fn control(&self, action: &str) -> Result<Value> {
        let db = self.database()?;
        let mut controls = db.controls()?;
        match action {
            "pause" => controls.paused = true,
            "resume" => {
                controls.paused = false;
                controls.sync_now += 1;
            }
            "sync_now" => controls.sync_now += 1,
            "quit" => controls.quit = true,
            _ => {
                return Err(Error::new(
                    ErrorCode::InvalidConfig,
                    "Choose pause, resume, sync now or quit.",
                ));
            }
        }
        db.set("controls", &controls)?;
        Ok(json!({"controls":controls}))
    }
    pub async fn configure(
        &self,
        local_path: &str,
        remote_id: &str,
        poll_secs: u64,
        deletion_limit: usize,
    ) -> Result<Value> {
        let map: Value =
            serde_json::from_slice(&std::fs::read(config_directory().join("development.json"))?)?;
        let mut pair: PairConfig = serde_json::from_value(map["test_pair"].clone())?;
        let path = if let Some(relative) = local_path.strip_prefix("~/") {
            directories::BaseDirs::new()
                .ok_or_else(internal)?
                .home_dir()
                .join(relative)
        } else {
            PathBuf::from(local_path)
        };
        let selected = local::canonical_root(&path)?;
        if selected != local::canonical_root(&pair.local_root)? || remote_id != pair.remote_root_id
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "This preview syncs only the mapped test-freesync folders. Adopting an existing tree is a later phase.",
            ));
        }
        pair.local_root = selected;
        pair.poll_secs = poll_secs;
        pair.deletion_limit = deletion_limit;
        self.request(EngineCommand::Configure { pair }).await
    }
    pub async fn browse(&self, parent: &str, page: Option<&str>) -> Result<Value> {
        let account = self
            .database()?
            .get::<String>("selected_account")?
            .map(Ok)
            .unwrap_or_else(bootstrap_account)?;
        let provider = GoogleDrive::saved(&account).await?;
        let parent_item = provider.get(parent).await?;
        if parent_item.kind != ItemKind::Folder || parent_item.trashed {
            return Err(Error::new(
                ErrorCode::InvalidConfig,
                "Choose an available Google Drive folder.",
            ));
        }
        let mut cursor = page.map(str::to_owned);
        let mut folders = vec![];
        let mut pages = 0;
        // Consume file-only pages too, without pretending the end of a folder list.
        while pages < 10 {
            let result = provider
                .children(&parent_item.id, cursor.as_deref())
                .await?;
            folders.extend(
                result
                    .items
                    .into_iter()
                    .filter(|i| i.kind == ItemKind::Folder)
                    .map(|i| json!({"id":i.id,"name":i.name,"writable":i.can_add_children})),
            );
            cursor = result.next;
            pages += 1;
            if cursor.is_none() || !folders.is_empty() {
                break;
            }
        }
        folders.sort_by(|a, b| {
            a["name"]
                .as_str()
                .unwrap_or_default()
                .to_lowercase()
                .cmp(&b["name"].as_str().unwrap_or_default().to_lowercase())
        });
        Ok(
            json!({"id":parent_item.id,"name":if parent=="root" {"My Drive"}else{&parent_item.name},"folders":folders,"next":cursor,"pages":pages}),
        )
    }
    pub async fn verify_account(&self) -> Result<Value> {
        let account = self
            .database()?
            .get::<String>("selected_account")?
            .map(Ok)
            .unwrap_or_else(bootstrap_account)?;
        let identity = GoogleDrive::saved(&account).await?.identity().await?;
        self.database()?.set("account_verified", &true)?;
        Ok(serde_json::to_value(identity)?)
    }
    pub async fn begin_login(&self, account: &str) -> Result<Value> {
        if account.trim().is_empty() || !account.contains('@') {
            return Err(Error::new(
                ErrorCode::InvalidConfig,
                "Enter the Google account email you want to connect.",
            ));
        }
        if self.login_cancel.lock().map_err(|_| internal())?.is_some() {
            return Err(Error::new(
                ErrorCode::InvalidConfig,
                "Finish or cancel the current Google sign-in first.",
            ));
        }
        let pending = PendingLogin::begin(
            &config_directory().join("google-client.json"),
            account.trim(),
        )
        .await?;
        let url = pending.url.clone();
        *self.login.lock().await = Some(pending);
        *self.login_cancel.lock().map_err(|_| internal())? = Some(CancellationToken::new());
        Ok(json!({"authorization_url":url}))
    }
    pub async fn finish_login(&self) -> Result<Value> {
        let pending =
            self.login.lock().await.take().ok_or_else(|| {
                Error::new(ErrorCode::InvalidConfig, "Start Google sign-in first.")
            })?;
        let token = self
            .login_cancel
            .lock()
            .map_err(|_| internal())?
            .as_ref()
            .ok_or_else(internal)?
            .clone();
        let result = pending.finish(Arc::new(NativeStore), token).await;
        self.login_cancel.lock().map_err(|_| internal())?.take();
        let account = result?;
        let db = self.database()?;
        db.set("selected_account", &account)?;
        db.set("account_verified", &true)?;
        Ok(json!({"account":account,"connected":true}))
    }
    pub async fn cancel_login(&self) -> Result<Value> {
        if let Some(token) = self.login_cancel.lock().map_err(|_| internal())?.take() {
            token.cancel();
        }
        self.login.lock().await.take();
        Ok(json!({"cancelled":true}))
    }
}
