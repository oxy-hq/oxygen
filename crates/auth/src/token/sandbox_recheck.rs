//! Admit a sandbox agent token **again**, by its id, for work it queued
//! earlier (sandbox agent credential design §4).
//!
//! A request that presents the token is admitted when it arrives. A check run
//! the token queued starts later, on a worker, with no request and no secret:
//! the task carries the token's id. Before the run starts the worker asks
//! here, and gets exactly what presenting the token now would give — the same
//! row, grants, expiry, revocation and minter checks, through the same
//! admission (`store::resolve_row`). Nothing is cached on this path.

use oxy_shared::errors::OxyError;
use sea_orm::DatabaseConnection;
use uuid::Uuid;

use super::format::TokenFormat;
use super::sandbox;
use super::store::{self, Resolved};

/// The credential of sandbox agent token `token_id`, as it stands now. `Err`
/// when the token is gone, revoked, expired, not this kind, or its minter is
/// no longer active — every case in which presenting it would be refused.
pub async fn readmit(db: &DatabaseConnection, token_id: Uuid) -> Result<Resolved, OxyError> {
    let row = sandbox::find(db, token_id)
        .await?
        .ok_or_else(|| OxyError::AuthenticationError("no such sandbox agent token".to_string()))?;
    store::resolve_row(db, row, TokenFormat::SandboxAgent).await
}
