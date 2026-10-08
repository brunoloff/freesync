//! Google Drive provider and native OAuth support.
pub mod auth;
pub mod drive;
pub mod store;

pub use drive::GoogleDrive;

pub struct GoogleFactory;
#[async_trait::async_trait]
impl freesync_core::engine::ProviderFactory for GoogleFactory {
    async fn connect(
        &self,
        pair: &freesync_core::PairConfig,
    ) -> freesync_core::Result<std::sync::Arc<dyn freesync_core::Provider>> {
        Ok(std::sync::Arc::new(GoogleDrive::for_test_pair(pair).await?))
    }
}
