//! The handlers of `/api/admin/standing-tokens` — every personal token that
//! carries its owner's standing, `platform` or `partner`, for the staff who
//! answer for who holds staff access (API-tokens design, "Standing tokens, for
//! staff"). Mounted, behind `manage_platform_grants`, by
//! `admin::standing_tokens`.
//!
//! These are the strongest credentials there are, and an all-access one is
//! seen by nobody but its owner: it holds no grant in any org, so no org's
//! inventory lists it. Without this list the one lever on a lost laptop is
//! deactivating the whole user.
//!
//! Three things are decided, in this order, before any token is read:
//!
//! 1. **The capability**, at the mount: without `manage_platform_grants` the
//!    guard answers a bare 403.
//! 2. **A browser session.** The routes list and revoke other people's
//!    credentials, so a credential cannot call them: each handler takes
//!    [`SessionOnly<ManageTokens>`] first, the gate of `/api/user/tokens`, and
//!    a key or token gets its 403 `session_required`.
//! 3. **An unbounded grant** ([`require_unbounded`]). Scope does not filter
//!    rows here, as it does on the other staff lists; it refuses the caller.
//!    A standing token is a credential for the whole deployment, so there is
//!    no subset of them that belongs to a grant bounded to some orgs: revoking
//!    one ends its reach in every org, and its row names orgs and people the
//!    bounded grant may not see. Such a caller gets 403
//!    `unbounded_grant_required` on both routes, whatever id it names.
//!
//! A revoke goes through [`service::revoke`], the owner's own path: the same
//! `token.revoked` rows, recorded as the staff member on every org the token
//! reaches, and the same credential-cache invalidation after the commit.

use axum::Json;
use axum::extract::Path;
use entity::prelude::ApiTokens;
use oxy::database::client::establish_connection;
use oxy_app_core::audit::RequestActor;
use oxy_auth::extractor::SessionOnly;
use oxy_auth::token::standing;
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::ManageTokens;
use super::dto::TokenDto;
use super::error::TokenError;
use super::handlers::{TokenList, token_id};
use super::sandbox_staff::{REVOKED_BY_STAFF, bound, minter_labels};
use super::{service, view};

/// The most tokens one listing returns, newest first.
const LIST_LIMIT: u64 = 500;

/// Whether a caller whose platform grant is bounded to `orgs` — `None` when
/// it covers every org — may use these routes: an unbounded grant only. A
/// grant bounded to nothing, and one bounded to every org that exists today,
/// are both bounded.
fn admit(orgs: Option<&[Uuid]>) -> Result<(), TokenError> {
    match orgs {
        None => Ok(()),
        Some(_) => Err(TokenError::UnboundedGrantRequired),
    }
}

/// Refuse a caller whose platform grant is bounded to some orgs. Asked of the
/// caller alone, so the answer cannot tell one token id from another. A grant
/// that cannot be read is a 500 — never "unbounded".
async fn require_unbounded(
    db: &DatabaseConnection,
    actor: &RequestActor,
) -> Result<(), TokenError> {
    admit(bound(db, actor).await?.as_deref())
}

/// List every personal API token that carries staff or partner standing, newest first (browser session and an unbounded platform grant only)
pub async fn list_standing_tokens(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
) -> Result<Json<TokenList>, TokenError> {
    let db = establish_connection().await?;
    require_unbounded(&db, &actor).await?;
    let rows = standing::list(&db, LIST_LIMIT).await?;
    let owners = minter_labels(&db, &rows).await?;
    let tokens: Vec<TokenDto> = view::tokens(&db, &rows, &owners).await?;
    Ok(Json(TokenList { tokens }))
}

/// Revoke a personal API token that carries staff or partner standing, whoever owns it (browser session and an unbounded platform grant only).
///
/// There is deliberately no `may_delegate` fence: a Global Admin may revoke a
/// peer's token or the Global Owner's, because a lost laptop must be
/// containable by whoever is on call, and the owner mints another from a
/// browser session.
pub async fn revoke_standing_token(
    _: SessionOnly<ManageTokens>,
    actor: RequestActor,
    Path(id): Path<String>,
) -> Result<Json<TokenDto>, TokenError> {
    let db = establish_connection().await?;
    // Before the id is parsed or anything is looked up by it.
    require_unbounded(&db, &actor).await?;
    let id = token_id(&id)?;
    let row = standing::find(&db, id).await?.ok_or(TokenError::NotFound)?;
    // Idempotent: revoking a revoked token changes nothing and records nothing.
    service::revoke(&db, &actor, row, REVOKED_BY_STAFF).await?;
    // Read back by id alone: it was a standing token when it was revoked, and
    // the answer must not become a 404 if its owner dropped the standing since.
    let revoked = ApiTokens::find_by_id(id).one(&db).await?;
    let revoked = revoked.ok_or(TokenError::NotFound)?;
    let owners = minter_labels(&db, std::slice::from_ref(&revoked)).await?;
    let label = owners.get(&revoked.principal_user_id).cloned();
    Ok(Json(
        view::token(&db, &revoked, label.as_deref().unwrap_or_default()).await?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORG: Uuid = Uuid::from_u128(0xA);
    const ELSEWHERE: Uuid = Uuid::from_u128(0xB);

    #[test]
    fn a_grant_over_every_org_is_admitted() {
        assert!(admit(None).is_ok());
    }

    /// Bounded is bounded, however much it covers: there is no org whose
    /// membership of a grant's scope makes a deployment-wide credential that
    /// grant's to see or to end.
    #[test]
    fn a_grant_bounded_to_any_set_of_orgs_is_refused() {
        for orgs in [&[][..], &[ORG][..], &[ORG, ELSEWHERE][..]] {
            assert!(
                matches!(admit(Some(orgs)), Err(TokenError::UnboundedGrantRequired)),
                "{orgs:?}"
            );
        }
    }
}
