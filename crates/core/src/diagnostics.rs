//! Bounded operational summaries and a detailed private activity mirror.
//! Only whitelisted diagnostic fields enter either log, never credentials,
//! file contents, OAuth URLs or upload session objects.
use crate::{ErrorCode, Result};
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    EngineStarted,
    PairStateChanged,
    EngineStopped,
}
#[derive(Serialize)]
struct Event {
    at: u64,
    kind: EventKind,
    queued: usize,
    conflicts: usize,
    error_code: Option<ErrorCode>,
}
pub fn record(
    profile: &Path,
    kind: EventKind,
    queued: usize,
    conflicts: usize,
    error_code: Option<ErrorCode>,
) -> Result<()> {
    let path = profile.join("events.jsonl");
    if fs::metadata(&path).is_ok_and(|m| m.len() > 2 * 1024 * 1024) {
        fs::rename(&path, profile.join("events.previous.jsonl"))?;
    }
    let mut options = OpenOptions::new();
    options.append(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let event = Event {
        at: crate::executor::now(),
        kind,
        queued,
        conflicts,
        error_code,
    };
    let mut output = options.open(path)?;
    serde_json::to_writer(&mut output, &event)?;
    output.write_all(b"\n")?;
    Ok(())
}
pub fn activity(profile: &Path, entries: &[crate::activity::Entry]) -> Result<()> {
    let path = profile.join("activity.jsonl");
    for entry in entries {
        if fs::metadata(&path).is_ok_and(|m| m.len() > 2 * 1024 * 1024) {
            fs::rename(&path, profile.join("activity.previous.jsonl"))?;
        }
        let mut options = OpenOptions::new();
        options.append(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut output = options.open(&path)?;
        serde_json::to_writer(&mut output, entry)?;
        output.write_all(b"\n")?;
    }
    Ok(())
}
