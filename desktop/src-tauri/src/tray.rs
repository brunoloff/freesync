//! Linux uses StatusNotifierItem because Tauri's AppIndicator backend does not
//! deliver primary-click events. Other platforms retain Tauri's native tray.
use crate::{backend::AppState, platform_error};
use freesync_core::Result;
use std::sync::Arc;
use tauri::Manager;

pub fn toggle(app: &tauri::AppHandle) -> Result<()> {
    let window = app.get_webview_window("main").ok_or_else(platform_error)?;
    if window.is_visible().map_err(|_| platform_error())?
        && !window.is_minimized().map_err(|_| platform_error())?
    {
        window.hide().map_err(|_| platform_error())?;
        record(app, "Settings hidden by tray click");
    } else {
        crate::show(app)?;
    }
    Ok(())
}
fn record(app: &tauri::AppHandle, message: &str) {
    if let Ok(db) = app.state::<Arc<AppState>>().database() {
        let _ = db.record_activity(&freesync_core::activity::Entry::new(
            "desktop", "info", message,
        ));
    }
}
fn action(app: &tauri::AppHandle, id: &str) {
    let result = match id {
        "show" => crate::show(app).map(|_| ()),
        "quit" => {
            crate::shutdown(app.clone());
            Ok(())
        }
        "pause" | "resume" | "sync_now" => app.state::<Arc<AppState>>().control(id).map(|_| ()),
        _ => Ok(()),
    };
    if let Err(error) = result
        && let Ok(db) = app.state::<Arc<AppState>>().database()
    {
        let mut entry = freesync_core::activity::Entry::new("desktop", "error", error.message);
        entry.details.error_code = Some(error.code);
        let _ = db.record_activity(&entry);
    }
}
#[cfg(target_os = "linux")]
pub struct Controller {
    handle: ksni::Handle<LinuxTray>,
}
#[cfg(target_os = "linux")]
struct LinuxTray {
    app: tauri::AppHandle,
    label: String,
    paused: bool,
}
#[cfg(target_os = "linux")]
impl ksni::Tray for LinuxTray {
    fn id(&self) -> String {
        "freesync".into()
    }
    fn title(&self) -> String {
        "FreeSync".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        let image = crate::icon();
        let bytes = image
            .rgba()
            .chunks_exact(4)
            .flat_map(|p| [p[3], p[0], p[1], p[2]])
            .collect();
        vec![ksni::Icon {
            width: 32,
            height: 32,
            data: bytes,
        }]
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip {
            title: "FreeSync".into(),
            description: self.label.clone(),
            ..Default::default()
        }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        let app = self.app.clone();
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Err(error) = toggle(&handle)
                && let Ok(db) = handle.state::<Arc<AppState>>().database()
            {
                let mut e = freesync_core::activity::Entry::new("desktop", "error", error.message);
                e.details.error_code = Some(error.code);
                let _ = db.record_activity(&e);
            }
        });
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        let item = |label: &str, id: &'static str, enabled: bool| {
            StandardItem {
                label: label.into(),
                enabled,
                activate: Box::new(move |tray: &mut Self| action(&tray.app, id)),
                ..Default::default()
            }
            .into()
        };
        vec![
            item(&self.label, "status", false),
            item("Open settings", "show", true),
            item("Pause sync", "pause", !self.paused),
            item("Resume sync", "resume", self.paused),
            item("Sync now", "sync_now", true),
            item("Quit", "quit", true),
        ]
    }
}
#[cfg(target_os = "linux")]
pub fn setup(app: &tauri::AppHandle) -> Result<Controller> {
    use ksni::TrayMethods;
    let handle = tauri::async_runtime::block_on(
        LinuxTray {
            app: app.clone(),
            label: "FreeSync is starting".into(),
            paused: false,
        }
        .spawn(),
    )
    .map_err(|_| platform_error())?;
    Ok(Controller { handle })
}
#[cfg(target_os = "linux")]
impl Controller {
    pub fn available(&self) -> bool {
        !self.handle.is_closed()
    }
    pub async fn update(&self, label: &str, paused: bool) {
        let _ = self
            .handle
            .update(|tray| {
                tray.label = label.into();
                tray.paused = paused;
            })
            .await;
    }
}
#[cfg(not(target_os = "linux"))]
pub struct Controller {
    status: tauri::menu::MenuItem<tauri::Wry>,
    pause: tauri::menu::MenuItem<tauri::Wry>,
    resume: tauri::menu::MenuItem<tauri::Wry>,
    app: tauri::AppHandle,
}
#[cfg(not(target_os = "linux"))]
pub fn setup(app: &tauri::AppHandle) -> Result<Controller> {
    use tauri::{
        menu::{Menu, MenuItem},
        tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    };
    let status = MenuItem::with_id(app, "status", "FreeSync is starting", false, None::<&str>)
        .map_err(|_| platform_error())?;
    let open = MenuItem::with_id(app, "show", "Open settings", true, None::<&str>)
        .map_err(|_| platform_error())?;
    let pause = MenuItem::with_id(app, "pause", "Pause sync", true, None::<&str>)
        .map_err(|_| platform_error())?;
    let resume = MenuItem::with_id(app, "resume", "Resume sync", true, None::<&str>)
        .map_err(|_| platform_error())?;
    let now = MenuItem::with_id(app, "sync_now", "Sync now", true, None::<&str>)
        .map_err(|_| platform_error())?;
    let quit =
        MenuItem::with_id(app, "quit", "Quit", true, None::<&str>).map_err(|_| platform_error())?;
    let menu = Menu::with_items(app, &[&status, &open, &pause, &resume, &now, &quit])
        .map_err(|_| platform_error())?;
    TrayIconBuilder::with_id("freesync")
        .icon(crate::icon())
        .tooltip("FreeSync")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| action(app, event.id.as_ref()))
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = toggle(tray.app_handle());
            }
        })
        .build(app)
        .map_err(|_| platform_error())?;
    Ok(Controller {
        status,
        pause,
        resume,
        app: app.clone(),
    })
}
#[cfg(not(target_os = "linux"))]
impl Controller {
    pub fn available(&self) -> bool {
        self.app.tray_by_id("freesync").is_some()
    }
    pub async fn update(&self, label: &str, paused: bool) {
        let _ = self.status.set_text(label);
        let _ = self.pause.set_enabled(!paused);
        let _ = self.resume.set_enabled(paused);
        if let Some(tray) = self.app.tray_by_id("freesync") {
            let _ = tray.set_tooltip(Some(format!("FreeSync — {label}")));
        }
    }
}
