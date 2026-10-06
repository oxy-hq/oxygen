//! GitHub Actions OIDC trusted publishing (design §6).
//!
//! A CI job presents an OIDC JWT proving "I am a run in repo X on ref Y"; we
//! verify it and mint a short-lived, app-scoped publish credential. The customer
//! stores no secret.
//!
//! This module is layered so the security-critical part — the **claim matching** —
//! is a pure function with no network and no DB, and is exhaustively unit-tested:
//!
//!   * `verify_claims` — given the decoded claims and the set of publisher configs
//!     for the repo, returns the app ids whose config matches (a monorepo can
//!     publish several apps from one repo). Every match rule from the design lives
//!     there.
//!   * Signature verification (RS256 against GitHub's JWKS), audience pinning and
//!     `jti` single-use are the envelope.
//!
//! Both now live in `oxy_auth::github_oidc`, the one verifier this exchange
//! shares with trusted access (`POST /api/auth/oidc/exchange`). What stays here
//! is this exchange — audience `oxy-publish`, `app_publishers` rows, an
//! `oxypublish_` token — and the publisher registration routes. It is kept
//! exactly as it shipped: a trust policy with an `app_publish` grant is the
//! newer way to the same publish, and converting these rows is a later step.
//!
//! The rules that get platforms owned are exactly the ones NOT to leave out:
//! never match `sub` (immutable-format changeover), require the `environment`
//! claim, reject `pull_request_target`, require a github-hosted runner, and match
//! on the numeric `repository_owner_id` (the account-resurrection defence).

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use oxy_auth::github_oidc::{self, AUDIENCE_PUBLISH};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// The verifier — keys, envelope, `jti` burn and the pure claim decision — is
// shared with trusted access. These names are re-exported because this module
// was where they lived.
pub use oxy_auth::github_oidc::{
    GithubOidcClaims, OidcError, OidcReject, PublisherConfig, machine_identity, verify_claims,
};

/// The audience this exchange requires, and the only one it accepts. Trusted
/// access requires `oxy`; neither accepts the other's.
pub const OXY_OIDC_AUDIENCE: &str = AUDIENCE_PUBLISH;

/// Verify a GitHub Actions OIDC JWT for this exchange's audience and burn its
/// `jti`. See [`oxy_auth::github_oidc::verify`].
pub async fn verify_token(
    db: &DatabaseConnection,
    token: &str,
) -> Result<GithubOidcClaims, OidcError> {
    let keys = crate::server::api::github_oidc_keys::github_keys();
    github_oidc::verify_token(db, keys, token, OXY_OIDC_AUDIENCE).await
}

/// Load the publisher configs for a repo, for `verify_claims`. Narrowed by the
/// numeric owner id + repo name so the pure matcher sees only plausibly-relevant
/// rows.
pub async fn publishers_for_repo(
    db: &DatabaseConnection,
    repo_owner_id: i64,
    repo_name: &str,
) -> Result<Vec<PublisherConfig>, OidcError> {
    use entity::prelude::AppPublishers;
    use sea_orm::{ColumnTrait, QueryFilter};
    let rows = AppPublishers::find()
        .filter(entity::app_publishers::Column::RepoOwnerId.eq(repo_owner_id))
        .filter(entity::app_publishers::Column::RepoName.eq(repo_name))
        .all(db)
        .await
        .map_err(|e| OidcError::Db(e.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|r| PublisherConfig {
            app_id: r.app_id,
            repo_owner: r.repo_owner,
            repo_owner_id: r.repo_owner_id,
            repo_name: r.repo_name,
            workflow_ref: r.workflow_ref,
            environment: r.environment,
        })
        .collect())
}

/// How long an exchanged credential lives. Short — it exists only to carry one
/// publish from the CI job that just proved its identity.
const EXCHANGE_TTL_MINUTES: i64 = 15;

#[derive(Deserialize)]
pub struct ExchangeRequest {
    /// The app being published, "org-slug/app-slug". The token proves the *repo*;
    /// this names *which app* in it (a monorepo publishes several).
    pub app: String,
}

#[derive(Serialize)]
pub struct ExchangeResponse {
    /// The short-lived, app-scoped publish token. The CLI uses it as the bearer on
    /// the normal publish call. Returned once, never stored in plaintext.
    pub token: String,
    pub expires_at: String,
    /// The app the token is scoped to. Returned so a CI job never has to look
    /// one up: the run routes are keyed by app id, and the app-listing route a
    /// slug→id lookup would use is not something a publish token may reach.
    pub app_id: Uuid,
}

fn bad(status: StatusCode, msg: &str) -> (StatusCode, String) {
    (status, msg.to_string())
}

/// `POST /customer-apps/publish/oidc-exchange` — the trusted-publishing entry
/// point. **Unauthenticated by construction**: the OIDC JWT in the Authorization
/// header IS the credential. Verify it, confirm the token's repo is a registered
/// publisher for the named app, and mint a short-lived app-scoped token.
///
/// Consent is NOT checked here — it is re-checked at publish time (a session
/// between exchange and publish must not let a stale credential outlive a revoke).
/// The exchange only proves "this CI job may mint a credential for this app".
pub async fn oidc_exchange_handler(
    headers: HeaderMap,
    Json(req): Json<ExchangeRequest>,
) -> Result<Json<ExchangeResponse>, (StatusCode, String)> {
    let jwt = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| bad(StatusCode::UNAUTHORIZED, "missing bearer OIDC token"))?;

    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| bad(StatusCode::INTERNAL_SERVER_ERROR, &format!("db: {e}")))?;

    // 1. Verify the token envelope (signature, iss, aud, exp, jti single-use).
    let claims = verify_token(&db, jwt).await.map_err(|e| {
        tracing::warn!("oidc-exchange: token rejected: {e:?}");
        match e {
            OidcError::Replayed => bad(StatusCode::UNAUTHORIZED, "token already used"),
            OidcError::Db(m) => bad(StatusCode::INTERNAL_SERVER_ERROR, &m),
            _ => bad(StatusCode::UNAUTHORIZED, "invalid OIDC token"),
        }
    })?;

    // 2. Resolve the named app.
    let (org_slug, app_slug) = req
        .app
        .split_once('/')
        .ok_or_else(|| bad(StatusCode::BAD_REQUEST, "app must be 'org-slug/app-slug'"))?;
    let app = resolve_app_by_slugs(&db, org_slug, app_slug)
        .await
        .map_err(|e| bad(StatusCode::INTERNAL_SERVER_ERROR, &e))?
        .ok_or_else(|| bad(StatusCode::NOT_FOUND, "app not found"))?;

    // 3. Match the token's claims against the publishers registered for its repo.
    let owner_id: i64 = claims
        .repository_owner_id
        .parse()
        .map_err(|_| bad(StatusCode::UNAUTHORIZED, "malformed repository_owner_id"))?;
    let repo_name = claims
        .repository
        .split_once('/')
        .map(|(_owner, name)| name.to_string())
        .unwrap_or_default();
    let publishers = publishers_for_repo(&db, owner_id, &repo_name)
        .await
        .map_err(|e| bad(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e:?}")))?;
    let matched = verify_claims(&claims, &publishers).map_err(|e| {
        bad(
            StatusCode::FORBIDDEN,
            &format!("no trusted publisher: {e:?}"),
        )
    })?;

    // The token may match several apps (monorepo); it must match THE ONE being
    // published.
    if !matched.contains(&app.id) {
        return Err(bad(
            StatusCode::FORBIDDEN,
            "this workflow is not a registered publisher for this app",
        ));
    }

    // 4. Mint the app-scoped machine token.
    let minted = mint_app_scoped_token(&db, app.id, &machine_identity(&claims))
        .await
        .map_err(|e| bad(StatusCode::INTERNAL_SERVER_ERROR, &e))?;

    tracing::info!(app_id = %app.id, repo = %claims.repository, "oidc-exchange: minted app-scoped publish token");
    Ok(Json(minted))
}

async fn resolve_app_by_slugs(
    db: &DatabaseConnection,
    org_slug: &str,
    app_slug: &str,
) -> Result<Option<entity::apps::Model>, String> {
    use entity::prelude::{Apps, Organizations};
    let Some(org) = Organizations::find()
        .filter(entity::organizations::Column::Slug.eq(org_slug))
        .one(db)
        .await
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    Apps::find()
        .filter(entity::apps::Column::OrgId.eq(org.id))
        .filter(entity::apps::Column::Slug.eq(app_slug))
        .one(db)
        .await
        .map_err(|e| e.to_string())
}

/// Insert an app-scoped, expiring, creator-less token row and return its plaintext.
async fn mint_app_scoped_token(
    db: &DatabaseConnection,
    app_id: Uuid,
    identity: &str,
) -> Result<ExchangeResponse, String> {
    let generated = oxy_auth::app_publish_token_domain::generate_token();
    let expires_at =
        (chrono::Utc::now() + chrono::Duration::minutes(EXCHANGE_TTL_MINUTES)).fixed_offset();
    entity::app_publish_tokens::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        // The attested workflow identity — carried to the build it publishes.
        name: ActiveValue::Set(identity.to_string()),
        token_hash: ActiveValue::Set(generated.token_hash),
        token_prefix: ActiveValue::Set(generated.token_prefix),
        // No human — this is the machine principal (design §6, Option A).
        created_by: ActiveValue::Set(None),
        created_at: ActiveValue::Set(chrono::Utc::now().fixed_offset()),
        last_used_at: ActiveValue::Set(None),
        revoked_at: ActiveValue::Set(None),
        app_id: ActiveValue::Set(Some(app_id)),
        expires_at: ActiveValue::Set(Some(expires_at)),
    }
    .insert(db)
    .await
    .map_err(|e| e.to_string())?;
    Ok(ExchangeResponse {
        token: generated.plaintext,
        expires_at: expires_at.to_rfc3339(),
        app_id,
    })
}

// ── publisher registration (staff surface) ──────────────────────────────────
//
// Who may publish which app via OIDC is deliberately explicit: a publisher row is
// registered here, and only a matching workflow can then trust-publish. Bootstrap
// rule (matches crates.io/npm): the app must already exist — a leaked credential
// must not be able to squat a new app name.

#[derive(Deserialize)]
pub struct RegisterPublisherBody {
    pub repo_owner: String,
    /// GitHub's NUMERIC account id — the resurrection defence. The operator reads
    /// it from the org's GitHub settings (or the API); we never accept just a name.
    pub repo_owner_id: i64,
    pub repo_name: String,
    /// Default ".github/workflows/oxy-publish.yml" — what `oxyc init-ci` generates.
    pub workflow_ref: String,
    /// Required — the environment the publish job runs in, so it can be gated
    /// behind required-reviewers.
    pub environment: String,
}

#[derive(Serialize)]
pub struct PublisherDto {
    pub id: Uuid,
    pub app_id: Uuid,
    pub repo_owner: String,
    pub repo_owner_id: i64,
    pub repo_name: String,
    pub workflow_ref: String,
    pub environment: String,
    pub created_at: String,
}

impl From<entity::app_publishers::Model> for PublisherDto {
    fn from(m: entity::app_publishers::Model) -> Self {
        Self {
            id: m.id,
            app_id: m.app_id,
            repo_owner: m.repo_owner,
            repo_owner_id: m.repo_owner_id,
            repo_name: m.repo_name,
            workflow_ref: m.workflow_ref,
            environment: m.environment,
            created_at: m.created_at.to_rfc3339(),
        }
    }
}

/// `GET /customer-apps/{id}/publishers`
pub async fn list_publishers(
    axum::extract::Path(app_id): axum::extract::Path<Uuid>,
) -> Result<Json<Vec<PublisherDto>>, StatusCode> {
    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let rows = entity::prelude::AppPublishers::find()
        .filter(entity::app_publishers::Column::AppId.eq(app_id))
        .all(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(rows.into_iter().map(PublisherDto::from).collect()))
}

/// `POST /customer-apps/{id}/publishers`
pub async fn register_publisher(
    axum::extract::Path(app_id): axum::extract::Path<Uuid>,
    oxy_auth::extractor::AuthenticatedUserExtractor(actor): oxy_auth::extractor::AuthenticatedUserExtractor,
    Json(body): Json<RegisterPublisherBody>,
) -> Result<Json<PublisherDto>, StatusCode> {
    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // Bootstrap: the app must exist.
    if entity::prelude::Apps::find_by_id(app_id)
        .one(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let saved = entity::app_publishers::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app_id),
        repo_owner: ActiveValue::Set(body.repo_owner),
        repo_owner_id: ActiveValue::Set(body.repo_owner_id),
        repo_name: ActiveValue::Set(body.repo_name),
        workflow_ref: ActiveValue::Set(body.workflow_ref),
        environment: ActiveValue::Set(body.environment),
        created_by: ActiveValue::Set(Some(actor.id)),
        created_at: ActiveValue::NotSet,
    }
    .insert(&db)
    .await
    // The UNIQUE claim tuple makes a duplicate a 409, not a 500.
    .map_err(|_| StatusCode::CONFLICT)?;
    Ok(Json(PublisherDto::from(saved)))
}

/// `DELETE /customer-apps/{id}/publishers/{publisher_id}`
pub async fn delete_publisher(
    axum::extract::Path((app_id, publisher_id)): axum::extract::Path<(Uuid, Uuid)>,
) -> Result<StatusCode, StatusCode> {
    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    // Scope the delete to the `{app_id}` in the URL — otherwise the path segment
    // is decorative and a publisher could be removed via any app's path. Deleting
    // by (id AND app_id) means a mismatched app deletes nothing → 404.
    let res = entity::prelude::AppPublishers::delete_many()
        .filter(entity::app_publishers::Column::Id.eq(publisher_id))
        .filter(entity::app_publishers::Column::AppId.eq(app_id))
        .exec(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    if res.rows_affected == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}
