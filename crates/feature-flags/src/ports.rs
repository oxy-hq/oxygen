//! The flag cache's one driven port: how it gets a database connection.
//!
//! The cache opens its own connection at every load (the boot load and each
//! refresh tick), so a boot whose connect failed still self-heals on a later
//! tick. It used to call `oxy_platform::db::establish_connection` directly;
//! taking the connector as a port is what lets `oxy-feature-flags` sit beside
//! `oxy-platform` instead of above it (`internal-docs/crate-tiers.md`, stage B).
//!
//! The production implementation is that same function: any
//! `Fn() -> impl Future<Output = Result<DatabaseConnection, OxyError>>` is a
//! [`FlagDb`], so `oxy-server` passes `establish_connection` itself and
//! nothing wraps it; the cache holds it as an `Arc<dyn FlagDb>`.

use std::future::Future;

use async_trait::async_trait;
use oxy_shared::errors::OxyError;
use sea_orm::DatabaseConnection;

/// Opens the connection one flag load reads through.
#[async_trait]
pub trait FlagDb: Send + Sync {
    async fn connect(&self) -> Result<DatabaseConnection, OxyError>;
}

/// A connector function is a [`FlagDb`]; `establish_connection` is the one
/// production implementation.
#[async_trait]
impl<F, Fut> FlagDb for F
where
    F: Fn() -> Fut + Send + Sync,
    Fut: Future<Output = Result<DatabaseConnection, OxyError>> + Send,
{
    async fn connect(&self) -> Result<DatabaseConnection, OxyError> {
        self().await
    }
}
