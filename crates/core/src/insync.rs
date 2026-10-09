//! Read-only import of recorded unselected local paths. Never reads credentials.
use crate::{Error, ErrorCode, Result, adoption::literal_glob, local};
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use std::{collections::BTreeSet, path::Path};
/// Import exact paths, not guessed patterns or tombstones without a local name.
/// The caller must show the inherited selection to the user before activation.
pub fn exclusions(config: &Path, root: &Path, email: &str) -> Result<Vec<String>> {
    if !config.join("settings.db").is_file() {
        return Ok(Vec::new());
    }
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let settings = Connection::open_with_flags(config.join("settings.db"), flags)?;
    let id: Option<String> = settings
        .query_row(
            "SELECT id FROM accounts WHERE cloud='gd' AND lower(email)=lower(?1)",
            [email],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id else {
        return Ok(Vec::new());
    };
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(Error::new(
            ErrorCode::InvalidConfig,
            "The old InSync account identifier is unsupported.",
        ));
    }
    let candidates = [
        config.join("data").join(format!("{id}.db")),
        config.join("data").join(format!("gd-{id}.db")),
    ];
    let Some(file) = candidates.iter().find(|p| p.is_file()) else {
        return Ok(Vec::new());
    };
    let data = Connection::open_with_flags(file, flags)?;
    let mut statement=data.prepare("SELECT n.node_id FROM nodes n JOIN fs_items f ON f.node_id=n.node_id LEFT JOIN sync_choices s ON s.node_id=n.node_id WHERE f.fs_name IS NOT NULL AND (n.sync_flags=0 OR s.chosen_sync_flags=0)")?;
    let mut rules = BTreeSet::new();
    for node in statement.query_map([], |r| r.get::<_, i64>(0))? {
        let mut node = Some(node?);
        let mut seen = BTreeSet::new();
        let mut parts = Vec::new();
        let mut absolute = None;
        while let Some(id) = node {
            if !seen.insert(id) || seen.len() > 1024 {
                return Err(Error::new(
                    ErrorCode::IncompleteScan,
                    "The old InSync inventory has an invalid parent chain.",
                ));
            }
            let row:Option<(Option<i64>,Option<String>)>=data.query_row("SELECT n.parent_id,coalesce(f.fs_name,c.cl_name) FROM nodes n LEFT JOIN fs_items f ON f.node_id=n.node_id LEFT JOIN cl_items c ON c.node_id=n.node_id WHERE n.node_id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let Some((parent, Some(name))) = row else {
                break;
            };
            if parent.is_none() {
                absolute = Some(std::path::PathBuf::from(name));
                break;
            }
            if local::validate_relative(&name).is_err() || name.contains('/') {
                break;
            }
            parts.push(name);
            node = parent;
        }
        if let Some(base) = absolute {
            if local::canonical_root(&base).ok().as_deref() != Some(root) {
                continue;
            }
            parts.reverse();
            let relative = parts.join("/");
            if relative.is_empty() {
                continue;
            }
            if local::safe_join(root, &relative)?.exists() {
                rules.insert(literal_glob(&relative));
            }
        }
    }
    Ok(rules.into_iter().collect())
}
