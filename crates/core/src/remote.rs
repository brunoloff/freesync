use crate::{
    Baseline, Error, ErrorCode, ItemKind, Provider, RemoteInventory, Result,
    local::{Exclusions, validate_relative},
};
use std::collections::{BTreeMap, BTreeSet};

async fn traverse(
    provider: &dyn Provider,
    root: &str,
    exclusions: &Exclusions,
) -> Result<RemoteInventory> {
    let root_item = provider.get(root).await?;
    if root_item.trashed || root_item.kind != ItemKind::Folder {
        return Err(Error::new(
            ErrorCode::IncompleteScan,
            "The selected Drive root is unavailable. Sync is paused.",
        ));
    }
    let mut entries = BTreeMap::new();
    let mut visited = BTreeSet::new();
    let mut todo = vec![(String::new(), root.to_owned())];
    while let Some((path, parent)) = todo.pop() {
        if !visited.insert(parent.clone()) {
            return Err(Error::new(
                ErrorCode::IncompleteScan,
                "A remote folder cycle or multiple-parent alias was found.",
            ));
        }
        let mut page = None;
        loop {
            let result = provider.children(&parent, page.as_deref()).await?;
            for item in result.items {
                if item.trashed {
                    continue;
                }
                let relative = if path.is_empty() {
                    item.name.clone()
                } else {
                    format!("{path}/{}", item.name)
                };
                if exclusions.excludes(&relative) {
                    continue;
                }
                // An excluded name stays outside reconciliation, including unsupported names.
                validate_relative(&item.name)?;
                if item.name.contains('/') {
                    return Err(Error::new(
                        ErrorCode::Unsupported,
                        "A Drive filename contains a path separator. Rename it before syncing.",
                    ));
                }
                if item.kind == ItemKind::Folder {
                    todo.push((relative.clone(), item.id.clone()));
                }
                entries.entry(relative).or_insert_with(Vec::new).push(item);
            }
            page = result.next;
            if page.is_none() {
                break;
            }
        }
    }
    Ok(RemoteInventory {
        entries,
        ..Default::default()
    })
}

/// Establish a consistent scoped inventory without advancing a durable cursor early.
pub async fn snapshot(
    provider: &dyn Provider,
    root: &str,
    patterns: &[String],
    baselines: &[Baseline],
) -> Result<RemoteInventory> {
    snapshot_from_cursor(provider, root, patterns, baselines, None).await
}

/// Continue the previously committed feed while retaining full-scan recovery.
pub async fn snapshot_from_cursor(
    provider: &dyn Provider,
    root: &str,
    patterns: &[String],
    baselines: &[Baseline],
    committed_cursor: Option<&str>,
) -> Result<RemoteInventory> {
    let canonical_id = provider.get(root).await?.id;
    let root = canonical_id.as_str();
    let ignore = Exclusions::new(patterns)?;
    let mut cursor = match committed_cursor {
        Some(cursor) => cursor.to_owned(),
        None => provider.start_cursor().await?,
    };
    for _ in 0..5 {
        let mut inventory = traverse(provider, root, &ignore).await?;
        let mut ids: BTreeSet<String> = inventory
            .entries
            .values()
            .flatten()
            .map(|i| i.id.clone())
            .collect();
        ids.insert(root.into());
        ids.extend(baselines.iter().map(|b| b.remote.id.clone()));
        let mut dirty = false;
        loop {
            let changes = match provider.changes(&cursor).await {
                Ok(changes) => changes,
                Err(error)
                    if error.code == ErrorCode::IncompleteScan && committed_cursor.is_some() =>
                {
                    // Expired cursors require a fresh traversal, never an empty inventory.
                    cursor = provider.start_cursor().await?;
                    dirty = true;
                    break;
                }
                Err(error) => return Err(error),
            };
            for change in changes.changes {
                let relevant = ids.contains(&change.id)
                    || change
                        .item
                        .as_ref()
                        .is_some_and(|i| i.parents.iter().any(|p| ids.contains(p)));
                if relevant {
                    dirty = true;
                    ids.insert(change.id);
                }
            }
            if let Some(next) = changes.next {
                cursor = next;
            } else {
                cursor = changes.new_cursor.ok_or_else(|| {
                    Error::new(
                        ErrorCode::IncompleteScan,
                        "Google change feed did not provide a completed cursor.",
                    )
                })?;
                break;
            }
        }
        if dirty {
            continue;
        }
        let present: BTreeSet<_> = inventory
            .entries
            .values()
            .flatten()
            .map(|i| i.id.as_str())
            .collect();
        for baseline in baselines {
            if ignore.excludes(&baseline.path) || present.contains(baseline.remote.id.as_str()) {
                continue;
            }
            match provider.get(&baseline.remote.id).await {
                Ok(item) if item.trashed => {
                    inventory.confirmed_removed.insert(item.id);
                }
                Ok(item) => {
                    if ancestry(provider, &item.id, root).await? == Ancestry::Outside {
                        inventory.confirmed_removed.insert(item.id);
                    }
                }
                Err(e) if matches!(e.code, ErrorCode::NotFound | ErrorCode::Permission) => (),
                Err(e) => return Err(e),
            }
        }
        // Revalidate the selected root after pagination and missing-item checks.
        let final_root = provider.get(root).await?;
        if final_root.trashed || final_root.kind != ItemKind::Folder {
            return Err(Error::new(
                ErrorCode::IncompleteScan,
                "The selected Drive root changed during inventory.",
            ));
        }
        inventory.cursor = Some(cursor);
        return Ok(inventory);
    }
    Err(Error::new(
        ErrorCode::IncompleteScan,
        "The Drive folder keeps changing. Retry after the current activity settles.",
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ancestry {
    Inside,
    Outside,
    Unavailable,
}
pub async fn ancestry(provider: &dyn Provider, id: &str, root: &str) -> Result<Ancestry> {
    let mut visited = BTreeSet::new();
    let mut todo = vec![id.to_owned()];
    let mut unavailable = false;
    while let Some(id) = todo.pop() {
        if id == root {
            return Ok(Ancestry::Inside);
        }
        if !visited.insert(id.clone()) {
            continue;
        }
        if visited.len() > 128 {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "Remote ancestry is too deep or contains a cycle.",
            ));
        }
        match provider.get(&id).await {
            Ok(item) => todo.extend(item.parents),
            Err(e) if matches!(e.code, ErrorCode::NotFound | ErrorCode::Permission) => {
                unavailable = true
            }
            Err(e) => return Err(e),
        }
    }
    Ok(if unavailable {
        Ancestry::Unavailable
    } else {
        Ancestry::Outside
    })
}
