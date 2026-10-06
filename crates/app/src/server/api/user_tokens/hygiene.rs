//! The token sweeper's two jobs (API-tokens design §8 Phase 5), called from
//! `server::token_sweep` on the global worker's tick:
//!
//! - [`send_expiry_notices`] — the 7-day expiry mail, once per token per
//!   expiry, for personal tokens and legacy keys (to the owner) and
//!   service-account tokens (to the org's admins and owners);
//! - [`expire_unused_tokens`] — a new-format token nobody used for a year gets
//!   `expires_at = now`, recorded as `token.expired_unused`. Legacy keys are
//!   exempt.
//!
//! **Why here and not a `TaskSpec`.** Both are idempotent and resumable from
//! the database alone: the notice is claimed on its row before it is sent, an
//! expiry is a guarded `UPDATE`, and anything a pass does not reach waits for
//! the next one. They are bounded — [`hygiene::BATCH`] rows, and a time budget
//! on the mail — so they never hold the global driver's tick. A crash between
//! claiming and sending loses that one notice, never duplicates it.

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use entity::api_tokens;
use oxy_app_core::audit::AuditContext;
use oxy_auth::token::hygiene::{self, UNUSED_DAYS, UNUSED_REASON};
use sea_orm::{DatabaseConnection, TransactionTrait};
use serde_json::json;

use super::audit::{self as token_audit, Event};
use super::dto;
use super::error::TokenError;
use super::recipients;
use super::service::reach_of;
use crate::emails::token_expiring::{ExpiringEmail, send_expiring_email};
use crate::emails::token_mail;

/// How long one pass may spend mailing. The rest is sent ten minutes on.
const MAIL_BUDGET: Duration = Duration::from_secs(20);
/// How long one send may take before it counts as failed.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);

/// What a notice pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoticeReport {
    pub sent: usize,
    /// Claimed, but nobody can be mailed (an owner with no address).
    pub nobody: usize,
    /// No mail went out; the claim was given back for the next pass.
    pub failed: usize,
}

/// Mail one token's recipients. `Ok(false)`: there is nobody to mail.
/// `Err` only when no recipient was reached, so a retry duplicates nothing.
async fn notify(db: &DatabaseConnection, row: &api_tokens::Model) -> Result<bool, String> {
    let to = recipients::for_token(db, row)
        .await
        .map_err(|e| format!("{e:?}"))?;
    if to.emails.is_empty() {
        return Ok(false);
    }
    let Some(expires_at) = row.expires_at.map(DateTime::<Utc>::from) else {
        return Ok(false);
    };
    let audience = to.audience();
    let legacy = dto::is_legacy(row);
    let mail = ExpiringEmail {
        token_name: &row.name,
        display_prefix: &row.display_prefix,
        legacy,
        expires_at,
        audience,
        // A legacy API key is extended on its own page, not in the token list.
        extend_url: if legacy {
            token_mail::legacy_keys_url()
        } else {
            audience.settings_url()
        },
    };
    let mut reached = 0;
    let mut last_error = String::new();
    for email in &to.emails {
        match tokio::time::timeout(SEND_TIMEOUT, send_expiring_email(email, &mail)).await {
            Ok(Ok(())) => reached += 1,
            Ok(Err(e)) => last_error = e.to_string(),
            Err(_) => last_error = "timed out".to_string(),
        }
    }
    if reached == 0 {
        return Err(last_error);
    }
    Ok(true)
}

/// Send every expiry notice due at `now`, soonest expiry first, within the
/// pass's budget.
pub async fn send_expiry_notices(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<NoticeReport, TokenError> {
    let started = Instant::now();
    let mut report = NoticeReport::default();
    for row in hygiene::due_notices(db, now).await? {
        if started.elapsed() >= MAIL_BUDGET {
            break;
        }
        if !hygiene::claim_notice(db, row.id).await? {
            continue;
        }
        match notify(db, &row).await {
            Ok(true) => report.sent += 1,
            Ok(false) => report.nobody += 1,
            Err(e) => {
                tracing::warn!(token_id = %row.id, error = %e, "expiry notice not sent; retrying next pass");
                hygiene::release_notice(db, row.id).await?;
                report.failed += 1;
            }
        }
    }
    Ok(report)
}

/// Expire one unused token and record it, in one transaction. `false` when it
/// was used or extended since it was read.
async fn expire_one(
    db: &DatabaseConnection,
    candidate: &api_tokens::Model,
    now: DateTime<Utc>,
) -> Result<bool, TokenError> {
    let txn = db.begin().await?;
    let Some(row) = hygiene::expire_unused(&txn, candidate.id, now).await? else {
        return Ok(false);
    };
    let (old, new) = (
        token_audit::rfc3339(candidate.expires_at),
        token_audit::rfc3339(row.expires_at),
    );
    Event {
        action: token_audit::EXPIRED_UNUSED,
        token: &row,
        orgs: reach_of(&txn, &row).await?,
        detail: json!({
            "reason": UNUSED_REASON,
            "unused_days": UNUSED_DAYS,
            "last_used_at": token_audit::rfc3339(row.last_used_at),
            "old_expires_at": old,
            "new_expires_at": new,
        }),
        change: Some((json!({ "expires_at": old }), json!({ "expires_at": new }))),
    }
    .record_as_system(&txn, &AuditContext::default())
    .await?;
    txn.commit().await?;
    oxy_auth::token::cache::invalidate_token(row.id);
    Ok(true)
}

/// Expire every new-format token unused for [`UNUSED_DAYS`] at `now`. The row
/// stays — revived by Extend if its owner wants it — and a legacy key is
/// never touched. Returns how many were expired.
pub async fn expire_unused_tokens(
    db: &DatabaseConnection,
    now: DateTime<Utc>,
) -> Result<usize, TokenError> {
    let mut expired = 0;
    for candidate in hygiene::unused_candidates(db, now).await? {
        if expire_one(db, &candidate, now).await? {
            expired += 1;
        }
    }
    Ok(expired)
}
