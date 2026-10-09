//! Local .gitignore policy. Matchers and tracked paths are read once per snapshot;
//! no Git processes are spawned and no repository metadata is written.
use crate::{Error, ErrorCode, ItemKind, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

struct Node {
    matcher: Gitignore,
    repository: Option<PathBuf>,
    tracked: Arc<BTreeSet<PathBuf>>,
    case_insensitive: bool,
}
#[derive(Default)]
struct Cache {
    nodes: BTreeMap<PathBuf, Node>,
    stamps: BTreeMap<PathBuf, Option<(u64, std::time::SystemTime)>>,
    digests: BTreeMap<PathBuf, String>,
}
pub struct Rules {
    root: PathBuf,
    boundary: PathBuf,
    cache: Mutex<Cache>,
}
fn failure() -> Error {
    Error::new(
        ErrorCode::IncompleteScan,
        "Git ignore rules or the tracked-file index could not be read safely. Sync stopped; repair the repository or disable Respect .gitignore for this folder.",
    )
}
fn stamp(path: &Path) -> Result<Option<(u64, std::time::SystemTime)>> {
    match fs::symlink_metadata(path) {
        Ok(m) => Ok(Some((m.len(), m.modified()?))),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(_) => Err(failure()),
    }
}
impl Rules {
    pub fn new(root: &Path) -> Result<Self> {
        let root = crate::local::canonical_root(root)?;
        let mut boundary = root.clone();
        // Only inherit rules outside the selected root when it actually lies
        // within a Git work tree. Empty .git markers in unrelated ancestors
        // (including protected workspace placeholders) are not repositories.
        for ancestor in root.ancestors() {
            let marker = ancestor.join(".git");
            if let Ok(meta) = fs::symlink_metadata(&marker)
                && !meta.file_type().is_symlink()
                && (meta.is_file() || marker.join("HEAD").is_file())
            {
                boundary = ancestor.into();
                break;
            }
        }
        Ok(Self {
            root,
            boundary,
            cache: Mutex::new(Cache::default()),
        })
    }

    fn load(cache: &mut Cache, dir: &Path, boundary: &Path) -> Result<()> {
        if cache.nodes.contains_key(dir) {
            return Ok(());
        }
        if dir != boundary
            && let Some(parent) = dir.parent()
        {
            Self::load(cache, parent, boundary)?;
        }
        let parent = dir.parent().and_then(|p| cache.nodes.get(p));
        let mut repository = parent.and_then(|p| p.repository.clone());
        let mut tracked = parent.map(|p| p.tracked.clone()).unwrap_or_default();
        let mut case_insensitive = parent.is_some_and(|p| p.case_insensitive);
        let marker = dir.join(".git");
        let marker_stamp = stamp(&marker)?;
        cache.stamps.insert(marker.clone(), marker_stamp);
        if marker_stamp.is_some() && (marker.is_file() || marker.join("HEAD").is_file()) {
            if fs::symlink_metadata(&marker)?.file_type().is_symlink() {
                return Err(failure());
            }
            let repo = git2::Repository::open(dir).map_err(|_| failure())?;
            if repo.workdir().is_none_or(|p| p != dir) {
                return Err(failure());
            }
            case_insensitive = repo
                .config()
                .map_err(|_| failure())?
                .get_bool("core.ignorecase")
                .unwrap_or(false);
            let config = repo.path().join("config");
            cache.stamps.insert(config.clone(), stamp(&config)?);
            let mut index = repo.index().map_err(|_| failure())?;
            if let Some(path) = index.path().map(Path::to_path_buf) {
                let before = stamp(&path)?;
                if before.is_some() {
                    cache
                        .digests
                        .insert(path.clone(), crate::local::fingerprint(&path)?.md5);
                }
                index.read(true).map_err(|_| failure())?;
                if stamp(&path)? != before {
                    return Err(failure());
                }
                cache.stamps.insert(path, before);
            }
            let mut paths = BTreeSet::new();
            for entry in index.iter() {
                let path = std::str::from_utf8(&entry.path).map_err(|_| failure())?;
                let path = dir.join(path);
                paths.insert(if case_insensitive {
                    PathBuf::from(path.to_string_lossy().to_ascii_lowercase())
                } else {
                    path
                });
            }
            tracked = Arc::new(paths);
            repository = Some(dir.to_path_buf());
        }
        let file = dir.join(".gitignore");
        let before = stamp(&file)?;
        let mut builder = GitignoreBuilder::new(dir);
        builder
            .case_insensitive(case_insensitive)
            .map_err(|_| failure())?;
        if before.is_some() && !fs::symlink_metadata(&file)?.file_type().is_symlink() {
            use md5::{Digest, Md5};
            let bytes = fs::read(&file)?;
            let text = std::str::from_utf8(&bytes).map_err(|_| failure())?;
            for (i, line) in text.lines().enumerate() {
                let line = if i == 0 {
                    line.trim_start_matches('\u{feff}')
                } else {
                    line
                };
                builder
                    .add_line(Some(file.clone()), line)
                    .map_err(|_| failure())?;
            }
            cache
                .digests
                .insert(file.clone(), format!("{:x}", Md5::digest(bytes)));
        }
        if stamp(&file)? != before {
            return Err(failure());
        }
        cache.stamps.insert(file, before);
        cache.nodes.insert(
            dir.into(),
            Node {
                matcher: builder.build().map_err(|_| failure())?,
                repository,
                tracked,
                case_insensitive,
            },
        );
        Ok(())
    }
    pub fn ignored(&self, relative: &str, kind: ItemKind) -> Result<bool> {
        crate::local::validate_relative(relative)?;
        let path = self.root.join(relative);
        let parent = path.parent().ok_or_else(failure)?;
        let mut cache = self.cache.lock().map_err(|_| failure())?;
        Self::load(&mut cache, parent, &self.boundary)?;
        // Git metadata is never excluded as a side effect of a broad ignore rule.
        if path.components().any(|c| c.as_os_str() == ".git") {
            return Ok(false);
        }
        let context = cache.nodes.get(parent).ok_or_else(failure)?;
        let tracked_path = if context.case_insensitive {
            PathBuf::from(path.to_string_lossy().to_ascii_lowercase())
        } else {
            path.clone()
        };
        if context.tracked.contains(&tracked_path)
            || kind == ItemKind::Folder
                && context
                    .tracked
                    .range(tracked_path.clone()..)
                    .next()
                    .is_some_and(|p| p.starts_with(&tracked_path))
        {
            return Ok(false);
        }
        let start = context
            .repository
            .clone()
            .unwrap_or_else(|| self.root.clone());
        // First evaluate parents: a child cannot re-include a path below an
        // excluded directory. At each level the nearest matching rule wins.
        let mut targets: Vec<PathBuf> = path
            .ancestors()
            .take_while(|p| **p != start)
            .map(Path::to_path_buf)
            .collect();
        targets.reverse();
        for target in targets {
            if !target.starts_with(&start) {
                continue;
            }
            let is_dir = target != path || kind == ItemKind::Folder;
            let mut dirs: Vec<_> = target
                .parent()
                .unwrap()
                .ancestors()
                .take_while(|p| p.starts_with(&start))
                .collect();
            dirs.reverse();
            let mut ignored = false;
            for dir in dirs {
                if let Some(node) = cache.nodes.get(dir) {
                    let matched = node.matcher.matched(&target, is_dir);
                    if !matched.is_none() {
                        ignored = matched.is_ignore();
                    }
                }
            }
            if ignored {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn signature(&self) -> Result<String> {
        use md5::{Digest, Md5};
        self.verify_unchanged()?;
        let cache = self.cache.lock().map_err(|_| failure())?;
        Ok(format!(
            "{:x}",
            Md5::digest(
                format!(
                    "{:?}:{:?}:{:?}",
                    cache.stamps,
                    cache.digests,
                    cache
                        .nodes
                        .iter()
                        .map(|(p, n)| (p, n.case_insensitive))
                        .collect::<Vec<_>>()
                )
                .as_bytes()
            )
        ))
    }
    pub fn verify_unchanged(&self) -> Result<()> {
        for (path, before) in &self.cache.lock().map_err(|_| failure())?.stamps {
            if &stamp(path)? != before {
                return Err(Error::new(
                    ErrorCode::IncompleteScan,
                    "Git ignore rules or the tracked-file index changed during the scan. Retry before syncing.",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_rules_parent_exclusion_negation_and_missing_remote_directories() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("src/deep")).unwrap();
        fs::write(
            root.path().join(".gitignore"),
            "/build/\n*.tmp\n!keep.tmp\nblocked/\n",
        )
        .unwrap();
        fs::write(root.path().join("src/.gitignore"), "!ok.tmp\n/deep/*.log\n").unwrap();
        let rules = Rules::new(root.path()).unwrap();
        for path in [
            "build",
            "build/arm64/file.txt",
            "src/x.tmp",
            "src/deep/x.log",
            "blocked/keep.tmp",
        ] {
            assert!(
                rules
                    .ignored(
                        path,
                        if path == "build" {
                            ItemKind::Folder
                        } else {
                            ItemKind::File
                        }
                    )
                    .unwrap(),
                "{path}"
            );
        }
        for path in ["keep.tmp", "src/ok.tmp", "src/build/file", "build"] {
            assert!(!rules.ignored(path, ItemKind::File).unwrap(), "{path}");
        }
        rules.verify_unchanged().unwrap();
        assert!(!rules.ignored("src/deep/keep.txt", ItemKind::File).unwrap());
        fs::write(root.path().join("src/deep/.gitignore"), "*.txt\n").unwrap();
        // Load records absence, so new nested rule files invalidate the snapshot.
        assert!(!rules.ignored("src/deep/keep.txt", ItemKind::File).unwrap());
        assert!(rules.verify_unchanged().is_err());
        assert!(
            Rules::new(root.path())
                .unwrap()
                .ignored("src/deep/keep.txt", ItemKind::File)
                .unwrap()
        );
    }
    #[test]
    fn indexed_files_and_ancestors_remain_included_even_inside_ignored_directories() {
        let root = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(root.path()).unwrap();
        fs::create_dir(root.path().join("build")).unwrap();
        fs::write(root.path().join("build/source.txt"), "tracked").unwrap();
        fs::write(root.path().join(".gitignore"), "/build/\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("build/source.txt")).unwrap();
        index.write().unwrap();
        let rules = Rules::new(root.path()).unwrap();
        assert!(!rules.ignored("build", ItemKind::Folder).unwrap());
        assert!(!rules.ignored("build/source.txt", ItemKind::File).unwrap());
        assert!(
            rules
                .ignored("build/generated.txt", ItemKind::File)
                .unwrap()
        );
        // A selected sync folder inside the repository still inherits its rules.
        let child = Rules::new(&root.path().join("build")).unwrap();
        assert!(!child.ignored("source.txt", ItemKind::File).unwrap());
        assert!(child.ignored("generated.txt", ItemKind::File).unwrap());
        index.remove_path(Path::new("build/source.txt")).unwrap();
        index.write().unwrap();
        assert!(rules.verify_unchanged().is_err());
    }
    #[test]
    fn malformed_or_unreadable_repository_metadata_stops_instead_of_expanding_sync() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join(".git")).unwrap();
        fs::write(root.path().join(".git/HEAD"), "invalid repository").unwrap();
        let rules = Rules::new(root.path()).unwrap();
        assert!(rules.ignored("secret.bin", ItemKind::File).is_err());
    }
    #[test]
    fn new_repositories_and_content_changes_with_restored_timestamps() {
        let root = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(root.path()).unwrap();
        repo.config()
            .unwrap()
            .set_bool("core.ignorecase", true)
            .unwrap();
        let path = root.path().join(".gitignore");
        fs::write(&path, "*.TMP\n").unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let rules = Rules::new(root.path()).unwrap();
        assert!(rules.ignored("cache.tmp", ItemKind::File).unwrap());
        let signature = rules.signature().unwrap();
        fs::write(root.path().join("TRACKED.tmp"), "tracked").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("TRACKED.tmp")).unwrap();
        index.write().unwrap();
        assert!(
            !Rules::new(root.path())
                .unwrap()
                .ignored("tracked.tmp", ItemKind::File)
                .unwrap()
        );
        fs::write(&path, "*.LOG\n").unwrap();
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let updated = Rules::new(root.path()).unwrap();
        assert!(!updated.ignored("cache.tmp", ItemKind::File).unwrap());
        assert!(updated.ignored("cache.log", ItemKind::File).unwrap());
        assert_ne!(signature, updated.signature().unwrap());
    }
}
