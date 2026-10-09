//! Continuous reconciliation with durable controls and per-pair status.
use crate::{db::Database, executor, profile, watcher::LocalWatcher, *};
use async_trait::async_trait;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

#[async_trait]
pub trait ProviderFactory: Send + Sync {
    async fn connect(&self, pair: &PairConfig) -> Result<Arc<dyn Provider>>;
}

/// Mutating settings requests execute between cycles on the sole profile owner.
/// UI/control connections cannot race the planner, transfer journal or recovery.
pub enum EngineCommand {
    Preview { pair_id: String },
    Activate { pair_id: String },
    Configure { pair: PairConfig },
    KeepBoth { pair_id: String, path: String },
    ApproveDeletions { pair_id: String, count: usize },
}
pub struct EngineRequest {
    pub command: EngineCommand,
    pub reply: oneshot::Sender<Result<serde_json::Value>>,
}
async fn handle_request(
    db: &mut Database,
    profile: &Path,
    factory: &dyn ProviderFactory,
    command: EngineCommand,
) -> Result<serde_json::Value> {
    let id = match &command {
        EngineCommand::Preview { pair_id }
        | EngineCommand::Activate { pair_id }
        | EngineCommand::KeepBoth { pair_id, .. }
        | EngineCommand::ApproveDeletions { pair_id, .. } => pair_id,
        EngineCommand::Configure { pair } => &pair.id,
    };
    let previous = db.pairs()?.into_iter().find(|p| &p.id == id);
    if let EngineCommand::Configure { mut pair } = command {
        if let Some(old) = previous
            && (old.local_root != pair.local_root
                || old.remote_root_id != pair.remote_root_id
                || old.account_email != pair.account_email
                || old.root_identity != pair.root_identity)
        {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "Changing an established pair's roots requires the adoption workflow.",
            ));
        }
        crate::local::check_nonoverlap(
            &pair.local_root,
            &db.pairs()?
                .into_iter()
                .filter(|p| p.id != pair.id)
                .map(|p| p.local_root)
                .collect::<Vec<_>>(),
            profile,
        )?;
        if !pair.test_only
            || !(1..=3600).contains(&pair.poll_secs)
            || !(1..=100_000).contains(&pair.deletion_limit)
        {
            return Err(Error::new(
                ErrorCode::InvalidConfig,
                "Choose valid polling and deletion limits for the test pair.",
            ));
        }
        factory.connect(&pair).await?;
        let inventory = crate::local::scan(&pair.local_root, &pair.excludes)?;
        if inventory.root_identity != pair.root_identity {
            return Err(Error::new(
                ErrorCode::IncompleteScan,
                "The configured local root changed. Re-select its original location.",
            ));
        }
        pair.enabled = false;
        db.save_pair(&pair)?;
        return Ok(serde_json::json!({"saved":true,"enabled":false}));
    }
    let mut pair = previous
        .ok_or_else(|| Error::new(ErrorCode::InvalidConfig, "Choose a configured folder pair."))?;
    let provider = factory.connect(&pair).await?;
    match command {
        EngineCommand::Preview { .. } => {
            let mut controls = db.controls()?;
            controls.paused = true;
            db.set("controls", &controls)?;
            let plan = crate::reconcile::prepare(db, &pair, provider.as_ref()).await?;
            Ok(crate::planner::preview(&plan))
        }
        EngineCommand::Activate { .. } => {
            crate::reconcile::prepare(db, &pair, provider.as_ref()).await?;
            pair.enabled = true;
            db.save_pair(&pair)?;
            let mut controls = db.controls()?;
            controls.paused = false;
            controls.sync_now += 1;
            db.set("controls", &controls)?;
            Ok(serde_json::json!({"activated":true}))
        }
        EngineCommand::KeepBoth { path, .. } => {
            let preserved = executor::keep_both(db, &pair, provider.as_ref(), &path).await?;
            Ok(serde_json::json!({"resolved":true,"preserved_local_path":preserved}))
        }
        EngineCommand::ApproveDeletions { count, .. } => {
            executor::approve_deletions(db, &pair, provider.as_ref(), count).await?;
            Ok(serde_json::json!({"reviewed_deletions":count,"fresh_inventory_verified":true}))
        }
        EngineCommand::Configure { .. } => unreachable!(),
    }
}
pub fn status(
    db: &Database,
    pair: &PairConfig,
    state: &str,
    error: Option<Error>,
) -> Result<PairStatus> {
    let queue = db.operations(&pair.id)?;
    let first = queue.first();
    Ok(PairStatus {
        pair_id: pair.id.clone(),
        state: state.into(),
        queued: queue.len(),
        conflicts: db.conflicts(&pair.id)?.len(),
        last_sync: db.status(&pair.id)?.and_then(|s| s.last_sync),
        error: error.or_else(|| first.and_then(|o| o.error.clone())),
        current_path: first.map(|o| o.path.clone()),
        progress_bytes: first
            .and_then(|o| o.upload_session.as_ref())
            .map_or(0, |s| s.uploaded_bytes),
        total_bytes: first
            .and_then(|o| o.upload_session.as_ref())
            .map_or(0, |s| s.total_bytes),
        retry_at: first
            .filter(|o| o.retry_at > executor::now())
            .map(|o| o.retry_at),
    })
}
pub async fn cycle(
    db: &mut Database,
    profile: &Path,
    pair: &PairConfig,
    factory: &dyn ProviderFactory,
    cancel: &CancellationToken,
) -> Result<()> {
    let provider = factory.connect(pair).await?;
    db.set_status(&status(db, pair, "scanning", None)?)?;
    crate::reconcile::prepare(db, pair, provider.as_ref()).await?;
    if db
        .get::<bool>(&format!("deletion_hold:{}", pair.id))?
        .unwrap_or(false)
    {
        return Err(Error::new(
            ErrorCode::Conflict,
            "A large deletion plan needs review and a fresh inventory before resuming.",
        ));
    }
    db.set_status(&status(db, pair, "syncing", None)?)?;
    executor::execute(db, profile, pair, provider.as_ref(), cancel, 1024 * 1024).await?;
    let queue = db.operations(&pair.id)?;
    let mut result = status(
        db,
        pair,
        if queue.is_empty() {
            if db.conflicts(&pair.id)?.is_empty() {
                "idle"
            } else {
                "conflicts"
            }
        } else {
            "waiting_retry"
        },
        None,
    )?;
    if queue.is_empty() {
        result.last_sync = Some(executor::now());
    }
    db.set_status(&result)
}
/// The caller holds ProfileLock. Separate control clients never start engines.
pub async fn run(
    db: &mut Database,
    profile: &Path,
    factory: &dyn ProviderFactory,
    cancel: CancellationToken,
) -> Result<()> {
    let (_sender, requests) = mpsc::channel(1);
    run_controlled(db, profile, factory, cancel, requests).await
}
pub async fn run_controlled(
    db: &mut Database,
    profile: &Path,
    factory: &dyn ProviderFactory,
    cancel: CancellationToken,
    mut requests: mpsc::Receiver<EngineRequest>,
) -> Result<()> {
    profile::private_directory(profile)?;
    db.record_activity(&crate::activity::Entry::new(
        "engine",
        "started",
        "Sync engine started; activity history is stored in this private profile",
    ))?;
    crate::diagnostics::record(
        profile,
        crate::diagnostics::EventKind::EngineStarted,
        0,
        0,
        None,
    )?;
    let mut controls = db.controls()?;
    controls.quit = false;
    db.set("controls", &controls)?;
    let mut seen_sync = controls.sync_now;
    let (sender, mut receiver) = mpsc::channel::<String>(32);
    let mut watchers: BTreeMap<String, (String, CancellationToken)> = BTreeMap::new();
    let mut due: BTreeMap<String, Instant> = BTreeMap::new();
    let mut dirty = BTreeSet::new();
    let mut previous_status = BTreeMap::new();
    loop {
        let controls = db.controls()?;
        if cancel.is_cancelled() || controls.quit {
            break;
        }
        while let Ok(request) = requests.try_recv() {
            let result = handle_request(db, profile, factory, request.command).await;
            let _ = request.reply.send(result);
        }
        let controls = db.controls()?;
        let pairs = db.pairs()?;
        crate::conflicts::process_tasks(db, profile, factory).await?;
        let enabled: BTreeSet<_> = pairs
            .iter()
            .filter(|p| p.enabled)
            .map(|p| p.id.clone())
            .collect();
        watchers.retain(|id, (_, token)| {
            if !enabled.contains(id) {
                token.cancel();
                false
            } else {
                true
            }
        });
        if controls.sync_now != seen_sync {
            dirty.extend(enabled.iter().cloned());
            seen_sync = controls.sync_now;
        }
        while let Ok(id) = receiver.try_recv() {
            dirty.insert(id);
        }
        for pair in &pairs {
            if !pair.enabled {
                db.set_status(&status(db, pair, "disabled", None)?)?;
                continue;
            }
            let signature =
                serde_json::to_string(&(&pair.local_root, &pair.excludes, pair.poll_secs))?;
            if watchers.get(&pair.id).is_none_or(|(s, _)| s != &signature) {
                if let Some((_, previous)) = watchers.remove(&pair.id) {
                    previous.cancel();
                }
                let cancellation = cancel.child_token();
                let worker_cancel = cancellation.clone();
                let tx = sender.clone();
                let config = pair.clone();
                tokio::spawn(async move {
                    loop {
                        match LocalWatcher::new(
                            &config.local_root,
                            &config.excludes,
                            Duration::from_secs(config.poll_secs.max(1)),
                            false,
                        ) {
                            Ok(mut watcher) => {
                                while watcher.wait(&worker_cancel).await {
                                    let _ = tx.try_send(config.id.clone());
                                }
                                break;
                            }
                            Err(_) => {
                                tokio::select! {_=worker_cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(config.poll_secs.max(1)))=>{let _=tx.try_send(config.id.clone());}}
                            }
                        }
                    }
                });
                watchers.insert(pair.id.clone(), (signature, cancellation));
                dirty.insert(pair.id.clone());
            }
            if controls.paused {
                db.set_status(&status(db, pair, "paused", None)?)?;
                continue;
            }
            if !dirty.remove(&pair.id)
                && due.get(&pair.id).is_some_and(|time| *time > Instant::now())
            {
                continue;
            }
            match cycle(db, profile, pair, factory, &cancel).await {
                Ok(()) => (),
                Err(e) if e.code == ErrorCode::Cancelled => {
                    db.set_status(&status(db, pair, "paused", None)?)?;
                }
                Err(e) => {
                    let state = match e.code {
                        ErrorCode::Authentication => "reconnect",
                        ErrorCode::Conflict => "needs_review",
                        ErrorCode::Transient | ErrorCode::RateLimited => "offline",
                        _ => "error",
                    };
                    db.set_status(&status(db, pair, state, Some(e))?)?;
                }
            }
            if let Some(status) = db.status(&pair.id)? {
                let signature = (
                    status.state,
                    status.queued,
                    status.conflicts,
                    status.error.map(|e| e.code),
                );
                if previous_status.get(&pair.id) != Some(&signature) {
                    crate::diagnostics::record(
                        profile,
                        crate::diagnostics::EventKind::PairStateChanged,
                        signature.1,
                        signature.2,
                        signature.3,
                    )?;
                    previous_status.insert(pair.id.clone(), signature);
                }
            }
            due.insert(
                pair.id.clone(),
                Instant::now() + Duration::from_secs(pair.poll_secs.max(1)),
            );
        }
        let log_error = db.flush_activity_log().err();
        db.set("activity_log_error", &log_error)?;
        tokio::select! {
            _=cancel.cancelled()=>break,
            Some(id)=receiver.recv()=>{dirty.insert(id);},
            Some(request)=requests.recv()=>{
                let result=handle_request(db, profile, factory, request.command).await;
                let _=request.reply.send(result);
            },
            _=tokio::time::sleep(Duration::from_millis(500))=>()
        }
    }
    for (_, (_, token)) in watchers {
        token.cancel();
    }
    for pair in db.pairs()? {
        db.set_status(&status(db, &pair, "stopped", None)?)?;
    }
    db.record_activity(&crate::activity::Entry::new(
        "engine",
        "completed",
        "Sync engine stopped cleanly",
    ))?;
    let log_error = db.flush_activity_log().err();
    db.set("activity_log_error", &log_error)?;
    crate::diagnostics::record(
        profile,
        crate::diagnostics::EventKind::EngineStopped,
        0,
        0,
        None,
    )?;
    Ok(())
}
