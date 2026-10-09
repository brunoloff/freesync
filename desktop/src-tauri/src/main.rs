#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod backend;
#[cfg(feature = "browser-test")]
mod browser_test;
mod desktop;
mod tray;
use backend::{AppState, Preferences};
use freesync_core::{Error, ErrorCode, Result, engine::EngineCommand};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tauri::Manager;
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

struct Shutdown(AtomicBool);
fn platform_error() -> Error {
    Error::new(
        ErrorCode::Io,
        "The desktop operation failed. Check your desktop session and try again.",
    )
}
fn show(app: &tauri::AppHandle) -> Result<Value> {
    let window = app.get_webview_window("main").ok_or_else(platform_error)?;
    window.unminimize().map_err(|_| platform_error())?;
    window.show().map_err(|_| platform_error())?;
    window.set_focus().map_err(|_| platform_error())?;
    if let Ok(db) = app.state::<Arc<AppState>>().database() {
        let _ = db.record_activity(&freesync_core::activity::Entry::new(
            "desktop",
            "info",
            "Settings shown",
        ));
    }
    Ok(json!({"visible":true}))
}
fn shutdown(app: tauri::AppHandle) {
    if app.state::<Shutdown>().0.swap(true, Ordering::SeqCst) {
        return;
    }
    let state = app.state::<Arc<AppState>>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = state.control("quit");
        match state.stop() {
            Ok(()) => app.exit(0),
            Err(_) => app.exit(1),
        }
    });
}
/// Shared dispatcher for native IPC and the explicitly enabled browser QA bridge.
/// No file contents, credential values, upload-session URLs or arbitrary commands
/// cross this boundary. The test bridge is excluded from normal builds.
pub async fn dispatch(app: tauri::AppHandle, command: &str, args: Value) -> Result<Value> {
    let result = dispatch_inner(app.clone(), command, args).await;
    // Record authored outcomes only, never OAuth arguments, arbitrary RPC data or URLs.
    let message = match command {
        "notify_test" => Some("Test notification requested"),
        "open_logs" => Some("Application logs folder opened"),
        "open_recovery" => Some("Recovery folder opened"),
        "preferences" => Some("Preferences updated"),
        _ => None,
    };
    let read = matches!(command, "snapshot" | "activity" | "folders");
    if !read
        && (message.is_some() || result.is_err())
        && let Ok(db) = app.state::<Arc<AppState>>().database()
    {
        let mut entry = match &result {
            Ok(_) => freesync_core::activity::Entry::new(
                "desktop",
                "info",
                message.unwrap_or("Desktop action completed"),
            ),
            Err(error) => {
                let mut e =
                    freesync_core::activity::Entry::new("desktop", "error", error.message.clone());
                e.details.error_code = Some(error.code);
                e
            }
        };
        if command == "notify_test" && result.is_ok() {
            entry.message = "Test notification submitted to the system notification service".into();
        }
        let _ = db.record_activity(&entry);
    }
    result
}
async fn dispatch_inner(app: tauri::AppHandle, command: &str, args: Value) -> Result<Value> {
    let state = app.state::<Arc<AppState>>().inner().clone();
    let text = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::InvalidConfig,
                    "A required settings value is missing.",
                )
            })
    };
    match command {
        "snapshot" => {
            let mut value = state.snapshot()?;
            value["window_visible"] = json!(
                app.get_webview_window("main")
                    .is_some_and(|w| w.is_visible().unwrap_or(false))
            );
            value["tray_available"] = json!(
                app.try_state::<tray::Controller>()
                    .is_some_and(|tray| tray.available())
            );
            value["autostart"] = json!(
                app.autolaunch()
                    .is_enabled()
                    .map_err(|_| platform_error())?
            );
            #[cfg(feature = "browser-test")]
            if cfg!(debug_assertions) && std::env::var_os("FREESYNC_BROWSER_TEST").is_some() {
                value["native_ui"] = json!(browser_test::native_view(&state.profile));
            }
            Ok(value)
        }
        "activity" => {
            let query: freesync_core::activity::Query = serde_json::from_value(args)?;
            let db = state.database()?;
            let mut value = serde_json::to_value(db.activity(&query)?)?;
            value["log_error"] =
                serde_json::to_value(db.get::<Option<Error>>("activity_log_error")?.flatten())?;
            Ok(value)
        }
        "control" => state.control(&text("action")?),
        "preview" => {
            state
                .request(EngineCommand::Preview {
                    pair_id: text("pairId")?,
                })
                .await
        }
        "activate" => {
            state
                .request(EngineCommand::Activate {
                    pair_id: text("pairId")?,
                })
                .await
        }
        "configure" => {
            state
                .configure(
                    &text("localPath")?,
                    &text("remoteId")?,
                    args["pollSecs"].as_u64().unwrap_or(10),
                    args["deletionLimit"].as_u64().unwrap_or(20) as usize,
                )
                .await
        }
        "browse" => {
            state
                .browse(&text("parent")?, args.get("page").and_then(Value::as_str))
                .await
        }
        "verify_account" => state.verify_account().await,
        "begin_login" => {
            let result = state.begin_login(&text("account")?).await?;
            // Browser QA opens this URL in its own controlled browser. Production
            // opens only the URL generated by our PKCE flow in the system browser.
            #[cfg(feature = "browser-test")]
            let browser_test =
                cfg!(debug_assertions) && std::env::var_os("FREESYNC_BROWSER_TEST").is_some();
            #[cfg(not(feature = "browser-test"))]
            let browser_test = false;
            if !browser_test {
                app.opener()
                    .open_url(
                        result["authorization_url"]
                            .as_str()
                            .ok_or_else(platform_error)?,
                        None::<&str>,
                    )
                    .map_err(|_| platform_error())?;
            }
            Ok(result)
        }
        "finish_login" => state.finish_login().await,
        "cancel_login" => state.cancel_login().await,
        "resolve_conflict" => state.queue_conflict(
            &text("pairId")?,
            &text("path")?,
            &text("conflictId")?,
            serde_json::from_value(args["choice"].clone())?,
        ),
        "keep_both" => {
            state.control("pause")?;
            state
                .request(EngineCommand::KeepBoth {
                    pair_id: text("pairId")?,
                    path: text("path")?,
                })
                .await
        }
        "approve_deletions" => {
            state
                .request(EngineCommand::ApproveDeletions {
                    pair_id: text("pairId")?,
                    count: args["count"].as_u64().ok_or_else(platform_error)? as usize,
                })
                .await
        }
        "preferences" => {
            if let Some(enabled) = args["autostart"].as_bool() {
                if enabled {
                    app.autolaunch().enable()
                } else {
                    app.autolaunch().disable()
                }
                .map_err(|_| platform_error())?;
            }
            if let Some(value) = args["notifications"].as_bool() {
                state
                    .database()?
                    .patch("preferences", &json!({"notifications":value}))?;
            }
            Ok(json!({"saved":true}))
        }
        "notify_test" => {
            desktop::notify(
                app,
                "This is a FreeSync test notification. Sync continues in the background.",
            )
            .await
        }
        "zoom" => {
            let level = args["level"]
                .as_f64()
                .filter(|v| v.is_finite() && (0.5..=2.0).contains(v))
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::InvalidConfig,
                        "Choose a zoom level between 50% and 200%.",
                    )
                })?;
            app.get_webview_window("main")
                .ok_or_else(platform_error)?
                .set_zoom(level)
                .map_err(|_| platform_error())?;
            state
                .database()?
                .patch("preferences", &json!({"zoom":level}))?;
            Ok(json!({"zoom":level}))
        }
        "open_recovery" | "open_logs" => {
            let directory = if command == "open_logs" {
                state.profile.clone()
            } else {
                state.profile.join("recovery")
            };
            if std::fs::symlink_metadata(&directory).is_ok_and(|m| m.file_type().is_symlink()) {
                return Err(platform_error());
            }
            freesync_core::profile::private_directory(&directory)?;
            let directory = directory.canonicalize()?;
            if !directory.starts_with(state.profile.canonicalize()?) {
                return Err(platform_error());
            }
            app.opener()
                .open_path(directory.to_string_lossy(), None::<&str>)
                .map_err(|_| platform_error())?;
            Ok(json!({"opened":true}))
        }
        "pick_folder" => {
            let application = app.clone();
            let selected = tauri::async_runtime::spawn_blocking(move || {
                application
                    .dialog()
                    .file()
                    .set_title("Choose local sync folder")
                    .blocking_pick_folder()
            })
            .await
            .map_err(|_| platform_error())?;
            Ok(json!({"path":selected.and_then(|p|p.into_path().ok())}))
        }
        "import_client" => {
            let application = app.clone();
            tauri::async_runtime::spawn_blocking(move || -> Result<Value> {
                let Some(selected) = application
                    .dialog()
                    .file()
                    .set_title("Import Desktop OAuth client")
                    .add_filter("Google Desktop client", &["json"])
                    .blocking_pick_file()
                else {
                    return Ok(json!({"cancelled":true}));
                };
                let path = selected.into_path().map_err(|_| platform_error())?;
                let metadata = std::fs::metadata(&path)?;
                if !metadata.is_file() || metadata.len() > 64 * 1024 {
                    return Err(Error::new(
                        ErrorCode::InvalidConfig,
                        "Choose a Google Desktop OAuth client JSON file.",
                    ));
                }
                let bytes = std::fs::read(path)?;
                let value: Value = serde_json::from_slice(&bytes)?;
                let client = &value["installed"];
                if client["auth_uri"] != "https://accounts.google.com/o/oauth2/auth"
                    || client["token_uri"] != "https://oauth2.googleapis.com/token"
                    || client["client_id"].as_str().is_none()
                    || client["client_secret"].as_str().is_none()
                {
                    return Err(Error::new(
                        ErrorCode::InvalidConfig,
                        "Choose a Desktop app client with Google's official OAuth endpoints.",
                    ));
                }
                let directory = freesync_google::auth::config_directory();
                freesync_core::profile::private_directory(&directory)?;
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create(true).truncate(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                use std::io::Write;
                options
                    .open(directory.join("google-client.json"))?
                    .write_all(&bytes)?;
                Ok(json!({"imported":true}))
            })
            .await
            .map_err(|_| platform_error())?
        }
        "hide" => {
            // CloseRequested is the same event used by the title-bar close action.
            app.get_webview_window("main")
                .ok_or_else(platform_error)?
                .close()
                .map_err(|_| platform_error())?;
            Ok(json!({"hidden":true,"engine_continues":true}))
        }
        "show" => show(&app),
        "quit" => {
            shutdown(app);
            Ok(json!({"stopping":true}))
        }
        _ => Err(Error::new(
            ErrorCode::InvalidConfig,
            "This settings action is not available.",
        )),
    }
}
#[tauri::command]
async fn settings(app: tauri::AppHandle, command: String, args: Value) -> Result<Value> {
    dispatch(app, &command, args).await
}
#[cfg(feature = "browser-test")]
#[tauri::command]
fn native_ui_probe(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    report: browser_test::NativeViewReport,
) -> Result<()> {
    if window.label() != "main" {
        return Err(platform_error());
    }
    // This command is deliberately absent from the browser dispatch endpoint.
    browser_test::record_native_view(&app, report)
}
fn icon() -> tauri::image::Image<'static> {
    // A code-native pair of synchronization arcs, matching the UI icon metaphor.
    let mut pixels = vec![0; 32 * 32 * 4];
    for y in 0..32 {
        for x in 0..32 {
            let dx = x as f64 - 15.5;
            let dy = y as f64 - 15.5;
            let radius = (dx * dx + dy * dy).sqrt();
            let arc = (10.0..=12.0).contains(&radius) && (dy.abs() > 3.0 || dx.abs() > 10.0);
            let arrow = (x >= 23 && y <= 12 && x + y >= 32 && x + y <= 38)
                || (x <= 8 && y >= 19 && x + y >= 23 && x + y <= 29);
            if arc || arrow {
                let i = (y * 32 + x) * 4;
                pixels[i..i + 4].copy_from_slice(&[38, 116, 76, 255]);
            }
        }
    }
    tauri::image::Image::new_owned(pixels, 32, 32)
}
fn main() {
    let state = match AppState::start() {
        Ok(state) => Arc::new(state),
        Err(error) => {
            eprintln!("{}", json!({"error":error}));
            std::process::exit(1);
        }
    };
    let builder = tauri::Builder::default()
        .manage(state.clone())
        .manage(Shutdown(AtomicBool::new(false)))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ));
    #[cfg(feature = "browser-test")]
    let builder = builder.invoke_handler(tauri::generate_handler![settings, native_ui_probe]);
    #[cfg(not(feature = "browser-test"))]
    let builder = builder.invoke_handler(tauri::generate_handler![settings]);
    let builder = builder
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                if let Ok(db) = window.app_handle().state::<Arc<AppState>>().database() {
                    let _ = db.record_activity(&freesync_core::activity::Entry::new(
                        "desktop",
                        "info",
                        "Settings closed; sync continues in the tray",
                    ));
                }
            }
        })
        .setup(|app| {
            let preferences: Preferences = app
                .state::<Arc<AppState>>()
                .database()?
                .get("preferences")?
                .unwrap_or_default();
            app.get_webview_window("main")
                .ok_or_else(platform_error)?
                .set_zoom(preferences.zoom.clamp(0.5, 2.0))?;
            app.manage(tray::setup(app.handle())?);
            if std::env::args().any(|a| a == "--background") {
                app.get_webview_window("main")
                    .ok_or_else(platform_error)?
                    .hide()?;
            }
            #[cfg(feature = "browser-test")]
            if cfg!(debug_assertions) && std::env::var_os("FREESYNC_BROWSER_TEST").is_some() {
                browser_test::start(app.handle().clone())?;
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut last_attention = String::new();
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    let state = handle.state::<Arc<AppState>>();
                    if state.stopped() {
                        shutdown(handle.clone());
                        break;
                    }
                    let _ = desktop::open_comparisons(&state);
                    if let Ok(snapshot) = state.snapshot() {
                        let paused = snapshot["controls"]["paused"].as_bool().unwrap_or(false);
                        let conflicts = snapshot["conflicts"].as_array().map_or(0, Vec::len);
                        let pairs = snapshot["pairs"].as_array().cloned().unwrap_or_default();
                        let working = pairs.iter().any(|p| {
                            matches!(p["status"]["state"].as_str(), Some("syncing" | "scanning"))
                        });
                        let error = pairs.iter().any(|p| {
                            matches!(
                                p["status"]["state"].as_str(),
                                Some("error" | "offline" | "reconnect" | "needs_review")
                            )
                        });
                        let label = if paused {
                            "Sync paused"
                        } else if error {
                            "Sync needs attention"
                        } else if conflicts > 0 {
                            "Conflicts need review"
                        } else if working {
                            "Syncing"
                        } else {
                            "Up to date"
                        };
                        handle
                            .state::<tray::Controller>()
                            .update(label, paused)
                            .await;
                        let attention = if error || conflicts > 0 { label } else { "" };
                        if !attention.is_empty()
                            && attention != last_attention
                            && snapshot["preferences"]["notifications"] == true
                        {
                            let _ = desktop::notify(
                                handle.clone(),
                                "Sync needs attention. Open settings to review the details.",
                            )
                            .await;
                        }
                        last_attention = attention.into();
                    }
                }
            });
            Ok(())
        });
    let application = builder.build(tauri::generate_context!());
    match application {
        Ok(application) => application.run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event
                && !app.state::<Arc<AppState>>().stopped()
            {
                api.prevent_exit();
                shutdown(app.clone());
            }
        }),
        Err(_) => {
            let _ = state.stop();
            eprintln!("{}", json!({"error":platform_error()}));
            std::process::exit(1);
        }
    }
}
