//! The warehouse the graph cores read through, as a seam.
//!
//! The cores compile semantic queries themselves, but running one needs a
//! connector built from the workspace's databases and secrets, scoped to the
//! caller. That builder is platform runtime in `oxy-app` (the connector pool
//! and the preview write hold), so the cores take it as this trait and
//! `oxy-app` implements it. Both of its callers do: the IDE routes, and the
//! customer-app routes that enter through the custom-app gate.

use std::sync::Arc;

use async_trait::async_trait;

/// One caller's view of a workspace's warehouses.
#[async_trait]
pub trait GraphWarehouse: Send + Sync {
    /// Run one statement against `database` on the platform's capped read
    /// path. Rows come back with the header row first, as the IDE's query
    /// endpoints return them.
    async fn query(&self, database: &str, sql: String) -> Result<Vec<Vec<String>>, String>;

    /// Open `database` for several statements, so a caller that runs many
    /// pays for the connector once.
    async fn connect(&self, database: &str) -> Result<Arc<dyn GraphConnection>, String>;
}

/// An open connection from [`GraphWarehouse::connect`].
#[async_trait]
pub trait GraphConnection: Send + Sync {
    /// The data rows of `sql`, header stripped. Empty when the statement
    /// fails: the failure is logged, and a node that cannot be valued shows
    /// no value rather than failing the whole response.
    async fn rows(&self, sql: &str) -> Vec<Vec<String>>;
}
