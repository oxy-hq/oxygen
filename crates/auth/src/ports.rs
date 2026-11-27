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

/// Where the session signing root's master-key text comes from
/// (`oxy_platform::secrets::master_key_text_for_signing` in production).
pub type MasterKeyText = fn() -> Option<String>;

/// Everything `oxy-auth` reaches the platform for: the connector each step
/// opens its connection through, and the master-key text `session_key` mixes
/// into the signing root. Built once by the host (`oxy::database::client::
/// auth_platform`) and handed to `AuthState`, `BuiltInAuthenticator` and the
/// entry points outside them. Cloning shares the connector.
#[derive(Clone)]
pub struct AuthPlatform {
    db: Arc<dyn AuthDb>,
    master_key_text: MasterKeyText,
}

impl AuthPlatform {
    pub fn new(db: impl AuthDb + 'static, master_key_text: MasterKeyText) -> Self {
        Self {
            db: Arc::new(db),
            master_key_text,
        }
    }

    /// The master-key text, read at the moment it is asked for (after boot
    /// has loaded a KMS key), exactly as the direct call read it.
    pub fn master_key_text_for_signing(&self) -> Option<String> {
        (self.master_key_text)()
    }
}

#[async_trait]
impl AuthDb for AuthPlatform {
    async fn connect(&self) -> Result<DatabaseConnection, OxyError> {
        self.db.connect().await
    }
}

/// A platform for this crate's unit tests, which open no database: a connect
/// fails with a database error, and there is no master key.
#[cfg(test)]
pub(crate) fn test_platform() -> AuthPlatform {
    AuthPlatform::new(
        || async {
            Err(OxyError::DBError(
                "no database in this unit test".to_string(),
            ))
        },
        || None,
    )
}
