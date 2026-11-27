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
