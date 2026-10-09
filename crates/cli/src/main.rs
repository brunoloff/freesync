use clap::{Parser, Subcommand};
use freesync_core::{
    Provider, Result,
    db::Database,
    local,
    profile::{ProfileLock, default_profile},
    watcher::LocalWatcher,
};
use freesync_google::{
    GoogleDrive,
    auth::{PendingLogin, bootstrap_account, config_directory},
    store::NativeStore,
};
use std::sync::Arc;
use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(
    name = "freesync",
    version,
    about = "Local-first desktop file synchronization"
)]
struct Cli {
    #[arg(long, global = true, env = "FREESYNC_PROFILE")]
    profile: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Inventory the mapped existing tree without transfers. Resume preserves verified hashes.
    Adopt {
        #[arg(long)]
        exclude: Vec<String>,
    },
    /// Read the saved adoption report (no scan or transfers).
    AdoptionReport {
        /// Reapply current local .gitignore rules to the saved inventory; no Drive requests.
        #[arg(long)]
        refresh_ignore_rules: bool,
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value = "")]
        name: String,
        #[arg(long, default_value_t = 0)]
        offset: u64,
    },
    /// Initialize an isolated application profile (does not sync files).
    Init,
    /// Inspect profile and pair status as JSON.
    Status,
    /// Run continuous sync for authorized pairs; SIGINT saves progress and stops.
    Run,
    /// Pause the current profile owner without starting another engine.
    Pause,
    /// Resume the current profile owner; stopped deletion plans still need review.
    Resume,
    /// Wake the running engine to reconcile immediately.
    SyncNow,
    /// Ask the running engine to save progress and stop.
    Quit,
    /// Activate a configured test pair after preparing its preview.
    Activate {
        #[arg(long, default_value = "test-freesync")]
        pair: String,
    },
    /// Resume a stopped deletion plan after reviewing the count and a fresh inventory.
    ApproveDeletions {
        #[arg(long, default_value = "test-freesync")]
        pair: String,
        #[arg(long)]
        count: usize,
    },
    /// Retry a stopped transfer with its original expectations and saved session.
    Retry {
        #[arg(long, default_value = "test-freesync")]
        pair: String,
        #[arg(long)]
        path: String,
    },
    /// Preserve both current versions of an ordinary file conflict.
    KeepBoth {
        #[arg(long, default_value = "test-freesync")]
        pair: String,
        #[arg(long)]
        path: String,
    },
    /// Persist and explain a dry-run plan for an existing folder pair.
    Plan {
        #[arg(long, default_value = "test-freesync")]
        pair: String,
    },
    /// Execute an authorized pair's queued plan once with durable transfer progress.
    SyncOnce {
        #[arg(long, default_value = "test-freesync")]
        pair: String,
        #[arg(long, default_value_t = 1048576)]
        chunk_size: usize,
    },
    /// Verify Google's stale-write rejection on one disposable test item.
    CheckWrites {
        #[arg(long)]
        id: String,
    },
    /// Revoke FreeSync's Google authorization and remove its saved credentials.
    Disconnect {
        #[arg(long)]
        account: Option<String>,
    },
    /// Provision and record the authorized disposable test folder pair, without enabling sync.
    BootstrapTest {
        #[arg(long)]
        local_root: PathBuf,
        #[arg(long)]
        account: Option<String>,
    },
    /// Read one Drive item's metadata and concurrency tag.
    Metadata {
        #[arg(long)]
        id: String,
        #[arg(long)]
        account: Option<String>,
    },
    /// Connect using a Desktop OAuth client; open the printed URL in your browser.
    Login {
        #[arg(long)]
        account: String,
        #[arg(long)]
        client_json: Option<PathBuf>,
    },
    /// Verify saved Google identity and optionally force token refresh.
    Account {
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        refresh: bool,
    },
    /// List a selected Drive folder, following all result pages.
    Browse {
        #[arg(long, default_value = "root")]
        parent: String,
        #[arg(long)]
        account: Option<String>,
        #[arg(long, default_value_t = 1000)]
        page_size: u32,
    },
    /// Inventory a selected remote root without writing Drive files.
    RemoteScan {
        #[arg(long)]
        root_id: String,
        #[arg(long)]
        account: Option<String>,
        #[arg(long)]
        exclude: Vec<String>,
    },
    /// Inventory a local folder without any cloud operations.
    Scan {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        exclude: Vec<String>,
    },
    /// Watch and rescan a folder; all operations are read-only.
    Watch {
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        exclude: Vec<String>,
        #[arg(long, default_value_t = 30)]
        poll_secs: u64,
        #[arg(long)]
        poll_only: bool,
    },
}
async fn execute(cli: Cli) -> Result<()> {
    let profile = cli.profile.unwrap_or_else(default_profile);
    match cli.command {
        Command::Adopt { exclude } => {
            let directory = profile.join("adoption");
            let _owner = ProfileLock::acquire(&directory)?;
            let manifest = freesync_core::adoption::Manifest::open(&directory)?;
            let source = if let Ok(mut saved) = manifest.source() {
                if !exclude.is_empty() {
                    saved.excludes = exclude;
                }
                freesync_google::inherit_adoption_exclusions(&mut saved)?;
                saved
            } else {
                freesync_google::adoption_source(exclude)?
            };
            manifest.initialize(&source)?;
            let provider = GoogleDrive::saved(&source.account_email).await?;
            let cancel = CancellationToken::new();
            let token = cancel.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                token.cancel();
            });
            let result = manifest.run(&provider, &cancel).await;
            println!(
                "{}",
                serde_json::to_string_pretty(&manifest.report(None, "", 0, 50)?)?
            );
            result?;
        }
        Command::AdoptionReport {
            refresh_ignore_rules,
            status,
            name,
            offset,
        } => {
            let manifest = freesync_core::adoption::Manifest::open(&profile.join("adoption"))?;
            let _lock = if refresh_ignore_rules {
                Some(ProfileLock::acquire(&profile.join("adoption"))?)
            } else {
                None
            };
            if refresh_ignore_rules {
                manifest.refresh_ignore_policy()?;
            }
            println!(
                "{}",
                serde_json::to_string_pretty(&manifest.report(
                    status.as_deref(),
                    &name,
                    offset,
                    100
                )?)?
            );
        }
        Command::KeepBoth { pair, path } => {
            let _owner = ProfileLock::acquire(&profile)?;
            let mut db = Database::open(&profile)?;
            let config = db
                .pairs()?
                .into_iter()
                .find(|p| p.id == pair)
                .ok_or_else(|| {
                    freesync_core::Error::new(
                        freesync_core::ErrorCode::InvalidConfig,
                        "Choose a configured folder pair.",
                    )
                })?;
            let provider = GoogleDrive::for_pair(&config, &profile).await?;
            let preserved =
                freesync_core::executor::keep_both(&mut db, &config, &provider, &path).await?;
            println!(
                "{}",
                serde_json::json!({"resolved":true,"preserved_local_path":preserved})
            );
        }
        Command::Retry { pair, path } => {
            let mut db = Database::open(&profile)?;
            let config = db
                .pairs()?
                .into_iter()
                .find(|p| p.id == pair)
                .ok_or_else(|| {
                    freesync_core::Error::new(
                        freesync_core::ErrorCode::InvalidConfig,
                        "Choose a configured folder pair.",
                    )
                })?;
            GoogleDrive::for_pair(&config, &profile).await?;
            db.retry_conflict(&pair, &path)?;
            let mut controls = db.controls()?;
            controls.sync_now += 1;
            db.set("controls", &controls)?;
            println!(
                "{}",
                serde_json::json!({"transfer_requeued":true,"original_expectations_retained":true})
            );
        }
        Command::ApproveDeletions { pair, count } => {
            let mut db = Database::open(&profile)?;
            let pair = db
                .pairs()?
                .into_iter()
                .find(|p| p.id == pair)
                .ok_or_else(|| {
                    freesync_core::Error::new(
                        freesync_core::ErrorCode::InvalidConfig,
                        "Choose a configured folder pair.",
                    )
                })?;
            let provider = GoogleDrive::for_pair(&pair, &profile).await?;
            freesync_core::executor::approve_deletions(&mut db, &pair, &provider, count).await?;
            let mut controls = db.controls()?;
            controls.sync_now += 1;
            db.set("controls", &controls)?;
            println!(
                "{}",
                serde_json::json!({"reviewed_deletions":count,"fresh_inventory_verified":true})
            );
        }
        Command::Run => {
            let _owner = ProfileLock::acquire(&profile)?;
            let mut db = Database::open(&profile)?;
            let cancel = CancellationToken::new();
            let signal = cancel.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                signal.cancel();
            });
            println!("{}", serde_json::json!({"engine_started":true}));
            freesync_core::engine::run(
                &mut db,
                &profile,
                &freesync_google::GoogleFactory {
                    profile: profile.clone(),
                },
                cancel,
            )
            .await?;
            println!("{}", serde_json::json!({"engine_stopped":true}));
        }
        Command::Pause | Command::Resume | Command::SyncNow | Command::Quit => {
            let db = Database::open(&profile)?;
            let mut controls = db.controls()?;
            match cli.command {
                Command::Pause => controls.paused = true,
                Command::Resume => {
                    controls.paused = false;
                    controls.sync_now += 1;
                }
                Command::SyncNow => controls.sync_now += 1,
                Command::Quit => controls.quit = true,
                _ => unreachable!(),
            }
            db.set("controls", &controls)?;
            println!("{}", serde_json::json!({"controls":controls}));
        }
        Command::Activate { pair } => {
            let _owner = ProfileLock::acquire(&profile)?;
            let mut db = Database::open(&profile)?;
            let mut pair = db
                .pairs()?
                .into_iter()
                .find(|p| p.id == pair)
                .ok_or_else(|| {
                    freesync_core::Error::new(
                        freesync_core::ErrorCode::InvalidConfig,
                        "Choose a configured folder pair.",
                    )
                })?;
            let drive = GoogleDrive::for_pair(&pair, &profile).await?;
            let plan = freesync_core::reconcile::prepare(&mut db, &pair, &drive).await?;
            pair.enabled = true;
            db.save_pair(&pair)?;
            println!(
                "{}",
                serde_json::json!({"activated":true,"preview":freesync_core::planner::preview(&plan)})
            );
        }
        Command::SyncOnce { pair, chunk_size } => {
            let _owner = ProfileLock::acquire(&profile)?;
            let mut db = Database::open(&profile)?;
            let pair = db
                .pairs()?
                .into_iter()
                .find(|p| p.id == pair)
                .ok_or_else(|| {
                    freesync_core::Error::new(
                        freesync_core::ErrorCode::InvalidConfig,
                        "Choose a configured folder pair.",
                    )
                })?;
            let drive = GoogleDrive::for_pair(&pair, &profile).await?;
            let cancellation = CancellationToken::new();
            let signal = cancellation.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                signal.cancel();
            });
            freesync_core::reconcile::prepare(&mut db, &pair, &drive).await?;
            let completed = freesync_core::executor::execute(
                &mut db,
                &profile,
                &pair,
                &drive,
                &cancellation,
                chunk_size,
            )
            .await?;
            println!(
                "{}",
                serde_json::json!({"completed":completed,"queued":db.operations(&pair.id)?.len(),"conflicts":db.conflicts(&pair.id)?.len()})
            );
        }
        Command::CheckWrites { id } => {
            let _owner = ProfileLock::acquire(&profile)?;
            let db = Database::open(&profile)?;
            let pair = db
                .pairs()?
                .into_iter()
                .find(|p| p.id == "test-freesync")
                .ok_or_else(|| {
                    freesync_core::Error::new(
                        freesync_core::ErrorCode::InvalidConfig,
                        "Configure the test pair first.",
                    )
                })?;
            GoogleDrive::for_pair(&pair, &profile)
                .await?
                .verify_conditional_write(&id)
                .await?;
            println!(
                "{}",
                serde_json::json!({"conditional_write_rejection_verified":true})
            );
        }
        Command::Plan { pair } => {
            let _owner = ProfileLock::acquire(&profile)?;
            let mut db = Database::open(&profile)?;
            let pair = db
                .pairs()?
                .into_iter()
                .find(|p| p.id == pair)
                .ok_or_else(|| {
                    freesync_core::Error::new(
                        freesync_core::ErrorCode::InvalidConfig,
                        "Choose a configured folder pair.",
                    )
                })?;
            let drive = GoogleDrive::saved(&pair.account_email).await?;
            let plan = freesync_core::reconcile::prepare(&mut db, &pair, &drive).await?;
            println!("{}", freesync_core::planner::preview(&plan));
        }
        Command::Disconnect { account } => {
            let _owner = ProfileLock::acquire(&profile)?;
            let db = Database::open(&profile)?;
            let account = account.map(Ok).unwrap_or_else(bootstrap_account)?;
            freesync_google::auth::Auth::saved(&account)
                .await?
                .disconnect()
                .await?;
            db.set("selected_account", &Option::<String>::None)?;
            println!("{}", serde_json::json!({"disconnected":true}));
        }
        Command::BootstrapTest {
            local_root,
            account,
        } => {
            if local_root.file_name().and_then(|s| s.to_str()) != Some("test-freesync") {
                return Err(freesync_core::Error::new(
                    freesync_core::ErrorCode::UnsafePath,
                    "The preview's test folder must be named test-freesync.",
                ));
            }
            let _owner = ProfileLock::acquire(&profile)?;
            let db = Database::open(&profile)?;
            std::fs::create_dir_all(&local_root)?;
            let local_root = local::canonical_root(&local_root)?;
            local::check_nonoverlap(
                &local_root,
                &db.pairs()?
                    .iter()
                    .filter(|p| p.id != "test-freesync")
                    .map(|p| p.local_root.clone())
                    .collect::<Vec<_>>(),
                &profile,
            )?;
            let account = account.map(Ok).unwrap_or_else(bootstrap_account)?;
            let drive = GoogleDrive::saved(&account).await?;
            let parent = drive.get("root").await?;
            let remote = drive.provision_test_root().await?;
            let inventory = local::scan(&local_root, &[])?;
            let pair = freesync_core::PairConfig {
                id: "test-freesync".into(),
                account_email: account.clone(),
                local_root: local_root.clone(),
                remote_root_id: remote.id.clone(),
                remote_root_name: remote.name.clone(),
                root_identity: inventory.root_identity,
                excludes: vec![],
                respect_gitignore: true,
                enabled: false,
                poll_secs: 10,
                deletion_limit: 20,
                test_only: true,
            };
            db.save_pair(&pair)?;
            let mapping = serde_json::json!({"schema_version":1,"account":account,"read_only_adoption":{"local_root":local_root.parent(),"remote_root_id":parent.id},"test_pair":pair});
            let path = config_directory().join("development.json");
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(path)?;
            file.write_all(serde_json::to_string_pretty(&mapping)?.as_bytes())?;
            println!(
                "{}",
                serde_json::json!({"test_pair_configured":true,"enabled":false,"local_count":inventory.entries.len(),"remote_root_verified":remote.parents.contains(&parent.id)})
            );
        }
        Command::Metadata { id, account } => {
            let account = account.map(Ok).unwrap_or_else(bootstrap_account)?;
            let drive = GoogleDrive::saved(&account).await?;
            println!("{}", serde_json::to_string(&drive.get(&id).await?)?);
        }
        Command::Login {
            account,
            client_json,
        } => {
            let _owner = ProfileLock::acquire(&profile)?;
            let pending = PendingLogin::begin(
                &client_json.unwrap_or_else(|| config_directory().join("google-client.json")),
                &account,
            )
            .await?;
            println!("{}", serde_json::json!({"authorization_url":pending.url}));
            let cancel = CancellationToken::new();
            let signal = cancel.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                signal.cancel();
            });
            let email = pending.finish(Arc::new(NativeStore), cancel).await?;
            Database::open(&profile)?.set("selected_account", &email)?;
            println!("{}", serde_json::json!({"connected":true,"account":email}));
        }
        Command::Account { account, refresh } => {
            let account = account.map(Ok).unwrap_or_else(bootstrap_account)?;
            let drive = GoogleDrive::saved(&account).await?;
            if refresh {
                drive.auth.bearer(true).await?;
            }
            let identity = drive.identity().await?;
            drive.start_cursor().await?;
            println!(
                "{}",
                serde_json::json!({"account":identity,"drive_access_verified":true,"refresh_verified":refresh})
            );
        }
        Command::Browse {
            parent,
            account,
            page_size,
        } => {
            let account = account.map(Ok).unwrap_or_else(bootstrap_account)?;
            let mut drive = GoogleDrive::saved(&account).await?;
            drive.page_size = page_size.clamp(1, 1000);
            let root = drive.get(&parent).await?;
            let mut pages = 0;
            let mut items = vec![];
            let mut cursor = None;
            loop {
                let result = drive.children(&parent, cursor.as_deref()).await?;
                pages += 1;
                items.extend(result.items);
                cursor = result.next;
                if cursor.is_none() {
                    break;
                }
            }
            println!(
                "{}",
                serde_json::json!({"root":root,"pages":pages,"items":items})
            );
        }
        Command::RemoteScan {
            root_id,
            account,
            exclude,
        } => {
            let account = account.map(Ok).unwrap_or_else(bootstrap_account)?;
            let drive = GoogleDrive::saved(&account).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &freesync_core::remote::snapshot(&drive, &root_id, &exclude, &[]).await?
                )?
            );
        }
        Command::Init => {
            let _owner = ProfileLock::acquire(&profile)?;
            let db = Database::open(&profile)?;
            println!(
                "{}",
                serde_json::json!({"initialized":true,"schema_version":db.schema_version()?})
            );
        }
        Command::Status => {
            let db = Database::open(&profile)?;
            let pairs = db.pairs()?;
            let statuses = pairs
                .iter()
                .map(|p| db.status(&p.id))
                .collect::<Result<Vec<_>>>()?;
            println!(
                "{}",
                serde_json::json!({"schema_version":db.schema_version()?,"pair_count":pairs.len(),"controls":db.controls()?,"statuses":statuses})
            );
        }
        Command::Scan { root, exclude } => println!(
            "{}",
            serde_json::to_string_pretty(&local::scan(&root, &exclude)?)?
        ),
        Command::Watch {
            root,
            exclude,
            poll_secs,
            poll_only,
        } => {
            let cancel = CancellationToken::new();
            let signal = cancel.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                signal.cancel();
            });
            let mut watcher = LocalWatcher::new(
                &root,
                &exclude,
                Duration::from_secs(poll_secs.max(1)),
                poll_only,
            )?;
            println!("{}", serde_json::json!({"watcher":watcher.backend()}));
            loop {
                match local::scan(&root, &exclude) {
                    Ok(inventory) => println!("{}", serde_json::to_string(&inventory)?),
                    Err(error) => println!("{}", serde_json::json!({"scan_error":error})),
                }
                if !watcher.wait(&cancel).await {
                    break;
                }
            }
        }
    }
    Ok(())
}
#[tokio::main]
async fn main() {
    if let Err(error) = execute(Cli::parse()).await {
        eprintln!("{}", serde_json::json!({"error":error}));
        std::process::exit(1);
    }
}
