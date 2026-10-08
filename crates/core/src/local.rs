use crate::{
    Error, ErrorCode, Fingerprint, ItemKind, LocalEntry, LocalInventory, Result, SkippedItem,
};
use globset::{Glob, GlobSet, GlobSetBuilder};
use md5::{Digest, Md5};
use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata},
    io::Read,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

pub struct Exclusions(GlobSet);
impl Exclusions {
    pub fn new(patterns: &[String]) -> Result<Self> {
        let mut b = GlobSetBuilder::new();
        for p in patterns {
            b.add(Glob::new(p).map_err(|_| {
                Error::new(ErrorCode::InvalidConfig, "An exclusion pattern is invalid.")
            })?);
        }
        Ok(Self(b.build().map_err(|_| {
            Error::new(
                ErrorCode::InvalidConfig,
                "Exclusion patterns could not be compiled.",
            )
        })?))
    }
    /// Excluding a directory also excludes its descendants, regardless of glob syntax.
    pub fn excludes(&self, path: &str) -> bool {
        let mut p = Path::new(path);
        loop {
            if self.0.is_match(p) {
                return true;
            }
            match p.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => p = parent,
                _ => return false,
            }
        }
    }
}

pub fn validate_relative(path: &str) -> Result<()> {
    if path.is_empty() || path.contains('\\') || path.contains('\0') || path.starts_with('/') {
        return Err(Error::new(
            ErrorCode::UnsafePath,
            "A relative filename is unsafe or unsupported.",
        ));
    }
    if path
        .split('/')
        .any(|c| c.is_empty() || c == "." || c == "..")
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(Error::new(
            ErrorCode::UnsafePath,
            "A filename escapes the selected folder.",
        ));
    }
    // Reject drive-prefix syntax even when checking a remote name on Unix.
    if path.split('/').any(|c| c.contains(':')) {
        return Err(Error::new(
            ErrorCode::Unsupported,
            "Filenames containing colons are not supported in this preview.",
        ));
    }
    Ok(())
}

pub fn canonical_root(root: &Path) -> Result<PathBuf> {
    if fs::symlink_metadata(root)?.file_type().is_symlink() || !root.is_dir() {
        return Err(Error::new(
            ErrorCode::UnsafePath,
            "Choose a real local directory, not a link.",
        ));
    }
    Ok(root.canonicalize()?)
}

pub fn check_nonoverlap(root: &Path, existing: &[PathBuf], state: &Path) -> Result<()> {
    let root = canonical_root(root)?;
    for other in existing.iter().chain(std::iter::once(&state.to_path_buf())) {
        let other = other.canonicalize()?;
        if root.starts_with(&other) || other.starts_with(&root) {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "Sync folders and application state must not overlap.",
            ));
        }
    }
    Ok(())
}

/// Validate each existing ancestor before any operation. Missing leaf paths are allowed.
pub fn safe_join(root: &Path, relative: &str) -> Result<PathBuf> {
    validate_relative(relative)?;
    let root = canonical_root(root)?;
    let mut candidate = root.clone();
    for c in relative.split('/') {
        candidate.push(c);
        match fs::symlink_metadata(&candidate) {
            Ok(m) if m.file_type().is_symlink() => {
                return Err(Error::new(
                    ErrorCode::UnsafePath,
                    "A link blocks this file operation.",
                ));
            }
            Ok(_) => {
                if !candidate.canonicalize()?.starts_with(&root) {
                    return Err(Error::new(
                        ErrorCode::UnsafePath,
                        "A path escapes the selected folder.",
                    ));
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(candidate)
}

pub fn identity(metadata: &Metadata) -> Option<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Some(format!("{}:{}", metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata
            .volume_serial_number()
            .zip(metadata.file_index())
            .map(|(v, i)| format!("{v}:{i}"))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = metadata;
        None
    }
}
fn modified(metadata: &Metadata) -> Result<u128> {
    metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .map_err(|_| {
            Error::new(
                ErrorCode::IncompleteScan,
                "A file has an unsupported modification date.",
            )
        })
}

pub fn fingerprint(path: &Path) -> Result<Fingerprint> {
    let before = fs::symlink_metadata(path)?;
    if !before.is_file() || before.file_type().is_symlink() {
        return Err(Error::new(
            ErrorCode::UnsafePath,
            "A file was replaced or is not an ordinary file.",
        ));
    }
    let mut file = File::open(path)?;
    if identity(&before) != identity(&file.metadata()?) {
        return Err(Error::new(
            ErrorCode::IncompleteScan,
            "A file changed while being opened. Retry after it settles.",
        ));
    }
    let mut digest = Md5::new();
    let mut bytes = [0u8; 65536];
    let mut size = 0;
    loop {
        let n = file.read(&mut bytes)?;
        if n == 0 {
            break;
        }
        digest.update(&bytes[..n]);
        size += n as u64;
    }
    let after = fs::symlink_metadata(path)?;
    if before.len() != size
        || after.len() != size
        || modified(&before)? != modified(&after)?
        || identity(&before) != identity(&after)
        || after.file_type().is_symlink()
    {
        return Err(Error::new(
            ErrorCode::IncompleteScan,
            "A file changed during the scan. Retry after it settles.",
        ));
    }
    Ok(Fingerprint {
        size,
        md5: format!("{:x}", digest.finalize()),
    })
}

pub fn entry(root: &Path, relative: &str) -> Result<Option<LocalEntry>> {
    let path = safe_join(root, relative)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let kind = if metadata.is_dir() {
        ItemKind::Folder
    } else if metadata.is_file() {
        ItemKind::File
    } else {
        return Err(Error::new(
            ErrorCode::Unsupported,
            "This filesystem item is not supported.",
        ));
    };
    Ok(Some(LocalEntry {
        path: relative.to_owned(),
        kind,
        fingerprint: if kind == ItemKind::File {
            Some(fingerprint(&path)?)
        } else {
            None
        },
        modified_ns: modified(&metadata)?,
        file_identity: identity(&metadata),
    }))
}

pub fn scan(root: &Path, patterns: &[String]) -> Result<LocalInventory> {
    let root = canonical_root(root)?;
    let root_before = fs::metadata(&root)?;
    let root_identity =
        identity(&root_before).unwrap_or_else(|| root.to_string_lossy().into_owned());
    let exclusions = Exclusions::new(patterns)?;
    let mut inventory = LocalInventory {
        root_identity: root_identity.clone(),
        entries: BTreeMap::new(),
        skipped: Vec::new(),
    };
    let mut todo = vec![String::new()];
    while let Some(dir) = todo.pop() {
        let path = if dir.is_empty() {
            root.clone()
        } else {
            safe_join(&root, &dir)?
        };
        let before = fs::metadata(&path)?;
        let mut children = fs::read_dir(&path)?.collect::<std::io::Result<Vec<_>>>()?;
        children.sort_by_key(|e| e.file_name());
        for child in children {
            let name = child.file_name().into_string().map_err(|_| {
                Error::new(
                    ErrorCode::IncompleteScan,
                    "A filename is not valid UTF-8. Rename it before syncing.",
                )
            })?;
            let relative = if dir.is_empty() {
                name
            } else {
                format!("{dir}/{name}")
            };
            if exclusions.excludes(&relative) {
                inventory.skipped.push(SkippedItem {
                    path: relative,
                    reason: "Excluded".into(),
                });
                continue;
            }
            validate_relative(&relative)?;
            let meta = fs::symlink_metadata(child.path())?;
            if meta.file_type().is_symlink() || !meta.is_file() && !meta.is_dir() {
                inventory.skipped.push(SkippedItem {
                    path: relative,
                    reason: "Links and special files are skipped".into(),
                });
                continue;
            }
            let value = entry(&root, &relative)?.ok_or_else(|| {
                Error::new(
                    ErrorCode::IncompleteScan,
                    "An item disappeared during the scan. Retry.",
                )
            })?;
            if value.kind == ItemKind::Folder {
                todo.push(relative.clone());
            }
            inventory.entries.insert(relative, value);
        }
        let after = fs::metadata(&path)?;
        if identity(&before) != identity(&after) || modified(&before)? != modified(&after)? {
            return Err(Error::new(
                ErrorCode::IncompleteScan,
                "A directory changed during the scan. Retry.",
            ));
        }
    }
    if identity(&root_before) != identity(&fs::metadata(&root)?) {
        return Err(Error::new(
            ErrorCode::IncompleteScan,
            "The selected root changed during the scan.",
        ));
    }
    Ok(inventory)
}
