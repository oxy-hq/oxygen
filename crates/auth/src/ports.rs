//! The driven port `oxy-auth` reaches the platform database through.
//!
//! Each auth step opens its connection at the point it always did — the
//! user lookups, the token re-read, the session-key load — and maps a failed
//! connect its own way. Taking the connector as a port, rather than calling
//! `oxy_platform::db::establish_connection` here, is what lets `oxy-auth` sit
//! beside `oxy-platform` instead of above it (`internal-docs/crate-tiers.md`,
//! stage B). Same shape as `oxy_feature_flags::ports::FlagDb`.
//!
//! The production implementation is that same function: any
//! `Fn() -> impl Future<Output = Result<DatabaseConnection, OxyError>>` is an
//! [`AuthDb`], so a caller passes `establish_connection` itself and nothing
//! wraps it.

use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;
use entity::platform_db::PlatformDb;
use oxy_shared::errors::OxyError;
use sea_orm::DatabaseConnection;

/// Opens the connection one auth step reads through.
#[async_trait]
pub trait AuthDb: Send + Sync {
    async fn connect(&self) -> Result<DatabaseConnection, OxyError>;
}

/// A connector function is an [`AuthDb`]; `establish_connection` is the one
/// production implementation.
#[async_trait]
impl<F, Fut> AuthDb for F
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Result<DatabaseConnection, OxyError>> + Send,
{
    async fn connect(&self) -> Result<DatabaseConnection, OxyError> {
        self().await
    }
}

/// Everything `oxy-auth` reaches the platform for: the connector each step
/// opens its connection through, and the master-key text `session_key` mixes
/// into the signing root. Both come from one kernel port,
/// [`entity::platform_db::PlatformDb`] (`oxy_platform::ProcessPlatformDb` in
/// production). Built once by the host (`oxy::database::client::
/// auth_platform`) and handed to `AuthState`, `BuiltInAuthenticator` and the
/// entry points outside them. Cloning shares the port.
#[derive(Clone)]
pub struct AuthPlatform {
    platform: Arc<dyn PlatformDb<Error = OxyError>>,
}

impl AuthPlatform {
    pub fn new(platform: impl PlatformDb<Error = OxyError> + 'static) -> Self {
        Self::from_shared(Arc::new(platform))
    }

    /// Builds on a port the host already shares.
    pub fn from_shared(platform: Arc<dyn PlatformDb<Error = OxyError>>) -> Self {
        Self { platform }
    }

    /// The master-key text, read at the moment it is asked for (after boot
    /// has loaded a KMS key), exactly as the direct call read it.
    pub fn master_key_text_for_signing(&self) -> Option<String> {
        PlatformDb::master_key_text_for_signing(&*self.platform)
    }
}

#[async_trait]
impl AuthDb for AuthPlatform {
    async fn connect(&self) -> Result<DatabaseConnection, OxyError> {
        PlatformDb::connect(&*self.platform).await
    }
}

/// A platform for this crate's unit tests, which open no database: a connect
/// fails with a database error, and there is no master key.
#[cfg(test)]
pub(crate) fn test_platform() -> AuthPlatform {
    struct NoDatabase;

    impl PlatformDb for NoDatabase {
        type Error = OxyError;

        fn connect(&self) -> entity::platform_db::ConnectFuture<'_, OxyError> {
            Box::pin(async {
                Err(OxyError::DBError(
                    "no database in this unit test".to_string(),
                ))
            })
        }

        fn master_key_text_for_signing(&self) -> Option<String> {
            None
        }

        fn seal(&self, _plaintext: &[u8]) -> Result<Vec<u8>, OxyError> {
            Err(OxyError::SecretManager(
                "no master key in this unit test".to_string(),
            ))
        }

        fn open(&self, _sealed: &[u8]) -> Result<Vec<u8>, OxyError> {
            Err(OxyError::SecretManager(
                "no master key in this unit test".to_string(),
            ))
        }
    }

    AuthPlatform::new(NoDatabase)
}
