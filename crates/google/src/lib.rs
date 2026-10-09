//! Google Drive provider and native OAuth support.
pub mod auth;
pub mod drive;
pub mod store;

pub use drive::GoogleDrive;

/// The private setup mapping pins My Drive's opaque ID; names are never identity.
pub fn adoption_source(
    excludes: Vec<String>,
) -> freesync_core::Result<freesync_core::adoption::Source> {
    use freesync_core::{Error, ErrorCode, local};
    let config: serde_json::Value = serde_json::from_slice(&std::fs::read(
        auth::config_directory().join("development.json"),
    )?)?;
    let root = std::path::PathBuf::from(
        config["read_only_adoption"]["local_root"]
            .as_str()
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::InvalidConfig,
                    "The existing-tree mapping is missing.",
                )
            })?,
    );
    let root = local::canonical_root(&root)?;
    let root_identity = local::identity(&std::fs::metadata(&root)?).ok_or_else(|| {
        Error::new(
            ErrorCode::Unsupported,
            "This filesystem cannot identify the adoption root.",
        )
    })?;
    let mut source = freesync_core::adoption::Source {
        account_email:auth::bootstrap_account()?,local_root:root,
        remote_root_id:config["read_only_adoption"]["remote_root_id"].as_str().ok_or_else(||Error::new(ErrorCode::InvalidConfig,"The existing Drive root mapping is missing."))?.into(),root_identity,excludes,
        exclusion_source:"No confirmed InSync exclusions are available; review the exclusions before activating a folder.".into(),
    };
    inherit_adoption_exclusions(&mut source)?;
    Ok(source)
}

/// Preserve recorded local selections without reading old-client credentials.
pub fn inherit_adoption_exclusions(
    source: &mut freesync_core::adoption::Source,
) -> freesync_core::Result<()> {
    let inherited = if let Some(dirs) = directories::BaseDirs::new() {
        freesync_core::insync::exclusions(
            &dirs.config_dir().join("Insync"),
            &source.local_root,
            &source.account_email,
        )?
    } else {
        Vec::new()
    };
    if !inherited.is_empty() {
        let noun = if inherited.len() == 1 {
            "path"
        } else {
            "paths"
        };
        source.exclusion_source = format!(
            "Carried over {} existing local {noun} recorded outside InSync's sync selection. No wildcard rules were guessed; review these exclusions before activation.",
            inherited.len()
        );
        source.excludes.extend(inherited);
        source.excludes.sort();
        source.excludes.dedup();
    }
    Ok(())
}

pub struct GoogleFactory {
    pub profile: std::path::PathBuf,
}
#[async_trait::async_trait]
impl freesync_core::engine::ProviderFactory for GoogleFactory {
    async fn connect(
        &self,
        pair: &freesync_core::PairConfig,
    ) -> freesync_core::Result<std::sync::Arc<dyn freesync_core::Provider>> {
        Ok(std::sync::Arc::new(
            GoogleDrive::for_pair(pair, &self.profile).await?,
        ))
    }
    async fn adoption_connect(
        &self,
        account: &str,
    ) -> freesync_core::Result<std::sync::Arc<dyn freesync_core::Provider>> {
        Ok(std::sync::Arc::new(GoogleDrive::saved(account).await?))
    }
}
