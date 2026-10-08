use async_trait::async_trait;
use freesync_core::{Error, ErrorCode, Result};

pub const SERVICE: &str = "freesync.google-drive";

#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn load(&self, account: &str) -> Result<Option<String>>;
    async fn save(&self, account: &str, value: &str) -> Result<()>;
    async fn delete(&self, account: &str) -> Result<()>;
}
pub struct NativeStore;
fn unavailable() -> Error {
    Error::new(
        ErrorCode::Authentication,
        "The OS credential store is unavailable or locked. Unlock it and reconnect Google Drive.",
    )
}

#[cfg(target_os = "linux")]
async fn service() -> Result<secret_service::SecretService<'static>> {
    secret_service::SecretService::connect(secret_service::EncryptionType::Dh)
        .await
        .map_err(|_| unavailable())
}
#[cfg(target_os = "linux")]
#[async_trait]
impl CredentialStore for NativeStore {
    async fn load(&self, account: &str) -> Result<Option<String>> {
        let service = service().await?;
        let collection = service
            .get_default_collection()
            .await
            .map_err(|_| unavailable())?;
        let items = collection
            .search_items(std::collections::HashMap::from([
                ("service", SERVICE),
                ("username", account),
            ]))
            .await
            .map_err(|_| unavailable())?;
        if items.len() > 1 {
            return Err(Error::new(
                ErrorCode::Authentication,
                "Duplicate FreeSync credentials were found. Reconnect this account.",
            ));
        }
        if let Some(item) = items.first() {
            if item.is_locked().await.map_err(|_| unavailable())? {
                return Err(unavailable());
            }
            let bytes = item.get_secret().await.map_err(|_| unavailable())?;
            Ok(Some(String::from_utf8(bytes).map_err(|_| unavailable())?))
        } else {
            Ok(None)
        }
    }
    async fn save(&self, account: &str, value: &str) -> Result<()> {
        let service = service().await?;
        let collection = service
            .get_default_collection()
            .await
            .map_err(|_| unavailable())?;
        if collection.is_locked().await.map_err(|_| unavailable())? {
            return Err(unavailable());
        }
        let attributes =
            std::collections::HashMap::from([("service", SERVICE), ("username", account)]);
        let items = collection
            .search_items(attributes.clone())
            .await
            .map_err(|_| unavailable())?;
        if items.len() > 1 {
            return Err(Error::new(
                ErrorCode::Authentication,
                "Duplicate FreeSync credentials were found. Reconnect this account.",
            ));
        }
        if let Some(item) = items.first() {
            item.set_secret(value.as_bytes(), "application/json")
                .await
                .map_err(|_| unavailable())?;
        } else {
            collection
                .create_item(
                    "FreeSync Google Drive",
                    attributes,
                    value.as_bytes(),
                    true,
                    "application/json",
                )
                .await
                .map_err(|_| unavailable())?;
        }
        Ok(())
    }
    async fn delete(&self, account: &str) -> Result<()> {
        let service = service().await?;
        let collection = service
            .get_default_collection()
            .await
            .map_err(|_| unavailable())?;
        for item in collection
            .search_items(std::collections::HashMap::from([
                ("service", SERVICE),
                ("username", account),
            ]))
            .await
            .map_err(|_| unavailable())?
        {
            item.delete().await.map_err(|_| unavailable())?;
        }
        Ok(())
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
#[async_trait]
impl CredentialStore for NativeStore {
    async fn load(&self, account: &str) -> Result<Option<String>> {
        match keyring::Entry::new(SERVICE, account)
            .map_err(|_| unavailable())?
            .get_password()
        {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(unavailable()),
        }
    }
    async fn save(&self, account: &str, value: &str) -> Result<()> {
        keyring::Entry::new(SERVICE, account)
            .map_err(|_| unavailable())?
            .set_password(value)
            .map_err(|_| unavailable())
    }
    async fn delete(&self, account: &str) -> Result<()> {
        match keyring::Entry::new(SERVICE, account)
            .map_err(|_| unavailable())?
            .delete_credential()
        {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(unavailable()),
        }
    }
}
