use crate::{
    Result,
    local::{Exclusions, canonical_root},
};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Events invalidate inventories. A periodic full scan covers missing events and overflow.
pub struct LocalWatcher {
    watcher: Option<RecommendedWatcher>,
    receiver: mpsc::Receiver<()>,
    root: PathBuf,
    exclusions: Vec<String>,
    poll: Duration,
    watched_root: bool,
}
impl LocalWatcher {
    pub fn new(
        root: &Path,
        exclusions: &[String],
        poll: Duration,
        force_poll: bool,
    ) -> Result<Self> {
        let root = canonical_root(root)?;
        Exclusions::new(exclusions)?;
        let (sender, receiver) = mpsc::channel(1);
        let event_root = root.clone();
        let ignore = Exclusions::new(exclusions)?;
        let mut watcher = if force_poll {
            None
        } else {
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                let relevant = match event {
                    Err(_) => true,
                    Ok(event) if event.need_rescan() => true,
                    // Inventory hashing opens files. Access notifications would
                    // feed those reads back into an endless reconciliation loop.
                    // Content/metadata changes still invalidate the inventory.
                    Ok(event) if matches!(event.kind, EventKind::Access(_)) => false,
                    Ok(event) => event.paths.iter().any(|p| {
                        p.strip_prefix(&event_root)
                            .map(|r| !ignore.excludes(&r.to_string_lossy().replace('\\', "/")))
                            // Parent watching detects this root's return; a
                            // changed sibling does not invalidate its contents.
                            .unwrap_or(false)
                    }),
                };
                if relevant {
                    let _ = sender.try_send(());
                }
            })
            .ok()
        };
        let mut watched_root = false;
        if let Some(w) = &mut watcher {
            watched_root = w.watch(&root, RecursiveMode::Recursive).is_ok();
            if let Some(parent) = root.parent() {
                let _ = w.watch(parent, RecursiveMode::NonRecursive);
            }
        }
        Ok(Self {
            watcher,
            receiver,
            root,
            exclusions: exclusions.to_vec(),
            poll,
            watched_root,
        })
    }
    pub fn backend(&self) -> &'static str {
        if self.watched_root {
            "native_with_rescan"
        } else {
            "polling"
        }
    }
    pub fn poll_interval(&self) -> Duration {
        self.poll
    }
    pub fn exclusions(&self) -> &[String] {
        &self.exclusions
    }
    /// Returns false on cancellation; true means reconcile, not "a file changed".
    pub async fn wait(&mut self, cancellation: &CancellationToken) -> bool {
        tokio::select! {
            _ = cancellation.cancelled() => return false,
            _ = tokio::time::sleep(self.poll) => (),
            Some(()) = self.receiver.recv(), if self.watcher.is_some() => {
                // Bounded debounce: continuous writes cannot starve periodic reconciliation.
                tokio::select! {
                    _ = cancellation.cancelled() => return false,
                    _ = tokio::time::sleep(Duration::from_millis(250)) => (),
                }
                while self.receiver.try_recv().is_ok() {}
            }
        }
        if let Some(w) = &mut self.watcher {
            if !self.root.is_dir() {
                self.watched_root = false;
            } else if !self.watched_root {
                self.watched_root = w.watch(&self.root, RecursiveMode::Recursive).is_ok();
            }
        }
        true
    }
}
