//! Read-only change diagnostics. Never prints file names, tokens or API URLs.
use freesync_core::{Error, ErrorCode, Provider, Result, adoption::Manifest, profile};
use freesync_google::GoogleDrive;
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        println!("{}", serde_json::json!({"error":error.code}));
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let manifest = Manifest::open(&profile::default_profile().join("adoption"))?;
    let source = manifest.source()?;
    let provider = GoogleDrive::saved(&source.account_email).await?;
    let mut cursor: String = manifest.get("cursor")?.ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidConfig,
            "No remote inventory cursor is saved yet.",
        )
    })?;
    let mut counts = std::collections::BTreeMap::<String, u64>::new();
    let mut metadata_details = Vec::new();
    let mut private_details = Vec::new();
    loop {
        let page = provider.changes(&cursor).await?;
        for change in page.changes {
            let old = manifest.observed_remote(&change.id)?;
            if std::env::var_os("FREESYNC_PRIVATE_CHANGE_REPORT").is_some() {
                let mut ancestors = Vec::new();
                let mut parent = change
                    .item
                    .as_ref()
                    .and_then(|item| item.parents.first())
                    .cloned();
                let mut seen = std::collections::BTreeSet::new();
                while let Some(id) = parent {
                    if !seen.insert(id.clone()) || seen.len() > 128 {
                        break;
                    }
                    let item = match manifest.observed_remote(&id)? {
                        Some(item) => item,
                        None => provider.get(&id).await?,
                    };
                    parent = item.parents.first().cloned();
                    ancestors.push(serde_json::json!({"id":item.id,"name":item.name}));
                }
                private_details
                    .push(serde_json::json!({"old":old,"new":change.item,"ancestors":ancestors}));
            }
            if let (Some(old), Some(new)) = (&old, &change.item)
                && !change.removed
                && old.name == new.name
                && old.parents == new.parents
                && old.kind == new.kind
                && old.trashed == new.trashed
                && old.fingerprint == new.fingerprint
            {
                let before = serde_json::to_value(old)?;
                let after = serde_json::to_value(new)?;
                let fields: Vec<_> = before
                    .as_object()
                    .unwrap()
                    .iter()
                    .filter(|(key, value)| after.get(*key) != Some(*value))
                    .map(|(key, _)| key.clone())
                    .collect();
                metadata_details.push(serde_json::json!({
                    "kind":new.kind,"changed_fields":fields,
                    "selected_root":new.id==source.remote_root_id,
                    "freesync_operation":new.operation_id.is_some()
                }));
            }
            let kind = match (change.removed, old, change.item) {
                (true, _, _) => "removed",
                (false, Some(old), Some(new))
                    if old.name != new.name
                        || old.parents != new.parents
                        || old.kind != new.kind
                        || old.trashed != new.trashed =>
                {
                    "structure_changed"
                }
                (false, Some(old), Some(new)) if old.fingerprint != new.fingerprint => {
                    "content_changed"
                }
                (false, Some(_), Some(_)) => "metadata_only",
                _ => "new_or_not_yet_listed",
            };
            *counts.entry(kind.into()).or_default() += 1;
        }
        if let Some(next) = page.next {
            cursor = next;
        } else {
            break;
        }
    }
    if let Some(path) = std::env::var_os("FREESYNC_PRIVATE_CHANGE_REPORT") {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options
            .open(path)?
            .write_all(&serde_json::to_vec_pretty(&private_details)?)?;
    }
    println!(
        "{}",
        serde_json::json!({"changes_since_current_pass_started":counts,"metadata_details":metadata_details})
    );
    Ok(())
}
