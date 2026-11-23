//! Read-only credentials for react runs ([`SystemPurpose::ReadOnlyQuery`]).
//!
//! Airhouse enforces a `read_only` credential on every statement (#256) and
//! echoes the claim. An Airhouse that predates it ignores the request field
//! and mints an ordinary Reader without saying so, so the echo is the only
//! proof: a credential without it is evicted and revoked, never handed out.
//! After a refusal the broker answers [`BrokerError::ReadOnlyUnsupported`] at
//! once for [`RECHECK_AFTER`], so an old Airhouse is not paid a mint and a
//! revoke on every react start, and an upgrade is noticed without a restart.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use tracing::warn;
use uuid::Uuid;

use super::{AirhouseTokenBroker, BrokerError, BrokerSubject, SystemPurpose};
use crate::admin::{EphemeralCredential, UserRole};

/// How long a refusal is trusted before Airhouse is asked again.
pub(super) const RECHECK_AFTER: Duration = Duration::from_secs(5 * 60);

/// When this broker last saw Airhouse ignore the claim.
#[derive(Debug, Default)]
pub(crate) struct ReadOnlyRefused(Mutex<Option<Instant>>);

impl ReadOnlyRefused {
    fn recent(&self) -> bool {
        let at = *self.0.lock().unwrap_or_else(PoisonError::into_inner);
        at.is_some_and(|at| at.elapsed() < RECHECK_AFTER)
    }

    fn mark(&self, refused: bool) {
        let mut at = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        *at = refused.then(Instant::now);
    }
}

impl AirhouseTokenBroker {
    /// A Reader credential that Airhouse confirmed may only read, for a react
    /// run's pooled query connection in `workspace_id`.
    pub async fn mint_read_only(
        &self,
        workspace_id: Uuid,
        ttl: Duration,
    ) -> Result<EphemeralCredential, BrokerError> {
        let subject = BrokerSubject::System {
            workspace_id,
            purpose: SystemPurpose::ReadOnlyQuery,
        };
        self.mint_read_only_checked(workspace_id, &subject, UserRole::Reader, ttl)
            .await
    }

    /// Every mint of a read-only subject, whichever entry point built it.
    pub(super) async fn mint_read_only_checked(
        &self,
        workspace_id: Uuid,
        subject: &BrokerSubject,
        role: UserRole,
        ttl: Duration,
    ) -> Result<EphemeralCredential, BrokerError> {
        if role != UserRole::Reader {
            return Err(BrokerError::ReadOnlyRole(role.as_str().to_string()));
        }
        if self.read_only_refused.recent() {
            return Err(BrokerError::ReadOnlyUnsupported);
        }
        let cred = self
            .mint_or_cached(workspace_id, subject, role, ttl)
            .await?;
        if cred.read_only {
            self.read_only_refused.mark(false);
            return Ok(cred);
        }
        self.read_only_refused.mark(true);
        self.evict(workspace_id, subject, role).await;
        if let Err(e) = self.revoke_user_token(workspace_id, &cred.username).await {
            warn!(
                workspace_id = %workspace_id,
                username = %cred.username,
                "could not revoke a Reader Airhouse minted without the read_only claim; it \
                 expires at {}: {e}",
                cred.expires_at
            );
        }
        Err(BrokerError::ReadOnlyUnsupported)
    }
}

#[cfg(test)]
#[path = "read_only_tests.rs"]
mod tests;
