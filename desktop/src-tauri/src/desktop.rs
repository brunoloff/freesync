use crate::{backend::AppState, platform_error};
use freesync_core::{ConflictChoice, Error, ErrorCode, Result, db::Database};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub async fn notify(app: tauri::AppHandle, body: &'static str) -> Result<Value> {
    #[cfg(target_os = "linux")]
    {
        let _ = app;
        tauri::async_runtime::spawn_blocking(move || {
            let inhibited=zbus::blocking::Connection::session().ok().and_then(|connection| {
                zbus::blocking::Proxy::new(&connection,"org.freedesktop.Notifications","/org/freedesktop/Notifications","org.freedesktop.Notifications").ok()?.get_property::<bool>("Inhibited").ok()
            });
            let handle=notify_rust::Notification::new().appname("FreeSync").summary("FreeSync").body(body).icon("folder-sync").timeout(8000).show().map_err(|_|Error::new(ErrorCode::Io,"The desktop notification service did not accept the notification. Check your desktop session and notification settings."))?;
            Ok(json!({"accepted":true,"notification_id":handle.id(),"inhibited":inhibited}))
        }).await.map_err(|_|platform_error())?
    }
    #[cfg(not(target_os = "linux"))]
    {
        use tauri_plugin_notification::NotificationExt;
        app.notification()
            .builder()
            .title("FreeSync")
            .body(body)
            .show()
            .map_err(|_| platform_error())?;
        Ok(json!({"requested":true,"inhibited":null}))
    }
}
fn on_path(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
    })
}
fn diff_program() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let programs = ["kdiff3", "meld", "opendiff"];
    #[cfg(target_os = "windows")]
    let programs = ["kdiff3.exe", "WinMergeU.exe"];
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let programs = ["kdiff3", "meld", "diffuse"];
    programs.into_iter().find_map(on_path)
}
fn diff_exit_ok(kdiff3: bool, code: Option<i32>) -> bool {
    // KDiff3Shell::closeEvent returns 1 when no merge file was saved, including
    // an ordinary read-only comparison. No output file is requested here.
    matches!(code, Some(0)) || kdiff3 && code == Some(1)
}
pub fn open_comparisons(state: &AppState) -> Result<()> {
    let db = state.database()?;
    for mut task in db
        .conflict_tasks()?
        .into_iter()
        .filter(|t| t.choice == ConflictChoice::Compare && t.state == "ready_to_open")
    {
        let launch = (|| -> Result<()> {
            let program=diff_program().ok_or_else(||Error::new(ErrorCode::Unsupported,"No system diff app was found. Install KDiff3, Meld, WinMerge or FileMerge and make it available on PATH."))?;
            // Some viewers hand off to a separate process. Keep their copies
            // rather than deleting files while that process may still use them.
            let cleanup_on_exit = program.file_stem().is_some_and(|name| name == "kdiff3");
            let directory = task
                .comparison_directory
                .as_ref()
                .ok_or_else(platform_error)?;
            let expected = state.profile.join("comparisons").join(&task.id);
            if directory != &expected
                || directory.canonicalize()? != expected
                || !directory.starts_with(state.profile.canonicalize()?)
            {
                return Err(platform_error());
            }
            for name in ["local.txt", "drive.txt"] {
                let path = directory.join(name);
                if path.canonicalize()? != path
                    || std::fs::symlink_metadata(&path)?.file_type().is_symlink()
                {
                    return Err(platform_error());
                }
            }
            let mut command = Command::new(program);
            if cleanup_on_exit {
                command
                    .arg("--L1")
                    .arg(format!("Local: {}", task.conflict.path))
                    .arg("--L2")
                    .arg(format!("Google Drive: {}", task.conflict.path));
            }
            let mut child = command
                .arg(directory.join("local.txt"))
                .arg(directory.join("drive.txt"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|_| Error::new(ErrorCode::Io, "The system diff app could not start."))?;
            task.state = "done".into();
            db.save_conflict_task(&task)?;
            db.set(&format!("diff_process:{}", task.id), &child.id())?;
            let task = task.clone();
            let profile = state.profile.clone();
            let directory = directory.clone();
            std::thread::spawn(move || {
                let result = child.wait();
                if !result.is_ok_and(|status| diff_exit_ok(cleanup_on_exit, status.code()))
                    && let Ok(db) = Database::open(&profile)
                {
                    let mut failed = task;
                    failed.state = "failed".into();
                    failed.error = Some(Error::new(
                        ErrorCode::Io,
                        "The system diff app exited with an error. Check its installation and try Diff again.",
                    ));
                    let _ = db.save_conflict_task(&failed);
                }
                if cleanup_on_exit {
                    remove_comparison(&directory);
                }
            });
            Ok(())
        })();
        if let Err(error) = launch {
            task.state = "failed".into();
            task.error = Some(error);
            db.save_conflict_task(&task)?;
        }
    }
    Ok(())
}
fn remove_comparison(directory: &Path) {
    #[cfg(target_os = "windows")]
    for name in ["local.txt", "drive.txt"] {
        if let Ok(metadata) = std::fs::metadata(directory.join(name)) {
            let mut permissions = metadata.permissions();
            permissions.set_readonly(false);
            let _ = std::fs::set_permissions(directory.join(name), permissions);
        }
    }
    let _ = std::fs::remove_dir_all(directory);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closing_a_readonly_kdiff3_comparison_is_not_a_failed_launch() {
        assert!(diff_exit_ok(true, Some(1)));
        assert!(diff_exit_ok(true, Some(0)));
        assert!(!diff_exit_ok(true, Some(2)));
        assert!(!diff_exit_ok(true, None));
        assert!(!diff_exit_ok(false, Some(1)));
    }
}
