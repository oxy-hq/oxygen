//! The entry point `oxy migrate` calls: [`super::open_at_boot`] against the
//! process's own database, outside Kubernetes nothing at all.
//!
//! Lowered from `oxy-app`'s `server::shared_state_check` on 2026-11-22
//! (decompose UP2), so that module could move up into the `oxy` binary while
//! `oxy migrate`, still `oxy-app`'s, reaches this without an upward edge.

use super::{open_at_boot, pod_namespace};
use crate::db::establish_connection;

/// For `oxy migrate`: in Kubernetes, open `server_keys` row `master_key` when
/// it is there. Never seals a new one.
pub async fn open_master_key() -> Result<(), String> {
    if pod_namespace().is_none() {
        return Ok(());
    }
    let db = master_key_db().await?;
    open_at_boot(&db).await
}

async fn master_key_db() -> Result<sea_orm::DatabaseConnection, String> {
    establish_connection()
        .await
        .map_err(|e| format!("Could not load the master key from the database: {e}"))
}
