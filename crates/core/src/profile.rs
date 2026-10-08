use crate::{Error, ErrorCode, Result};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

pub fn default_profile() -> PathBuf {
    directories::ProjectDirs::from("", "", "freesync")
        .map(|p| p.data_local_dir().join("profiles/default"))
        .unwrap_or_else(|| PathBuf::from(".freesync-profile"))
}

pub fn private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub struct ProfileLock {
    _file: File,
    pub directory: PathBuf,
}
impl ProfileLock {
    pub fn acquire(directory: &Path) -> Result<Self> {
        private_directory(directory)?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join("owner.lock"))?;
        file.try_lock_exclusive().map_err(|_| {
            Error::new(
                ErrorCode::LockBusy,
                "This profile is already running. Open its settings or stop the current owner.",
            )
        })?;
        Ok(Self {
            _file: file,
            directory: directory.canonicalize()?,
        })
    }
}
