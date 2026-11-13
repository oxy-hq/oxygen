//! The org lifetime cap on a token's **new** expiry (API-tokens design §3.6,
//! §5): create, extend and regenerate refuse an expiry beyond the tightest
//! cap among the orgs the token holds grants in, with 400
//! `{ code: "exceeds_policy", max_lifetime_days }`.
//!
//! The rule is `oxy_auth::token::policy::check_expiry` — the same lifetime the
//! request path enforces, `expires_at - created_at` — so a token this lets
//! through is never inert for its lifetime. Two shapes are never refused:
//!
//! - a **legacy key**, which no policy caps (§3.5);
//! - an **all-access** personal token, which reaches every org its owner is
//!   in: it is not refused, it goes inert in the capping orgs instead.

use chrono::{DateTime, Utc};
use entity::api_tokens;
use oxy_auth::token::StoredKind;
use oxy_auth::token::personal;
use oxy_auth::token::policy::{TokenShape, check_expiry};
use oxy_auth::token::policy_store;
use sea_orm::ConnectionTrait;
use uuid::Uuid;

use super::error::TokenError;

/// Refuse `expires_at` for a token of `shape` holding grants in `org_ids`.
pub(crate) async fn check<C: ConnectionTrait>(
    db: &C,
    shape: &TokenShape,
    org_ids: &[Uuid],
    expires_at: Option<DateTime<Utc>>,
) -> Result<(), TokenError> {
    if shape.legacy || shape.all_access_personal() || org_ids.is_empty() {
        return Ok(());
    }
    let cap = policy_store::tightest_cap_in(db, org_ids).await?;
    check_expiry(shape, expires_at, cap)
        .map_err(|max_lifetime_days| TokenError::ExceedsPolicy { max_lifetime_days })
}

/// A token about to be minted, as the cap reads it: created now.
pub(crate) fn new_token(kind: StoredKind, all_access: bool) -> TokenShape {
    TokenShape {
        kind,
        legacy: false,
        all_access,
        created_at: Utc::now(),
        expires_at: None,
    }
}

/// Refuse `expires_at` for a stored token, by the orgs of its live grants.
pub(crate) async fn check_row<C: ConnectionTrait>(
    db: &C,
    row: &api_tokens::Model,
    expires_at: Option<DateTime<Utc>>,
) -> Result<(), TokenError> {
    let Some(shape) = TokenShape::of(row) else {
        return Ok(());
    };
    if shape.legacy || shape.all_access_personal() {
        return Ok(());
    }
    let grants = personal::grants_for(db, &[row.id]).await?;
    check(db, &shape, &policy_store::grant_orgs(&grants), expires_at).await
}
