//! Registered token issuers: the writes and reads behind
//! `/api/orgs/{org_id}/oidc-issuers`, and the lookup the exchange starts from
//! (API-tokens design §3.4, *Any OIDC issuer*).
//!
//! Database primitives only: who may ask, the discovery fetch, and the audit
//! row are the handler's.
//!
//! An issuer is stored without a trailing `/`, and looked up by a token's
//! `iss` without one, so `https://x/` and `https://x` are the same issuer.
//!
//! An issuer belongs to one org. The exchange looks one up **only in the org
//! of the service account the request names**, by exact `issuer` — so an
//! issuer another org registered is never consulted, and a token's `iss` can
//! only select among issuers that org's admin chose.

use chrono::Utc;
use entity::oidc_issuers::{self, KEY_SOURCE_DISCOVERY, KEY_SOURCE_INLINE, KEY_SOURCE_JWKS_URI};
use entity::prelude::OidcIssuers;
use oxy_shared::errors::OxyError;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect, Set, SqlErr,
};
use serde_json::Value;
use uuid::Uuid;

use crate::workload_identity::normalized_issuer;

fn db_err(what: &'static str) -> impl FnOnce(DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// Where an issuer's keys come from, as registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssuerKeys {
    /// The `jwks_uri` its discovery document named, read at registration from
    /// `discovery_url` (`None` = the issuer itself).
    Discovery {
        discovery_url: Option<String>,
        jwks_uri: String,
    },
    /// A key-set URL the admin gave.
    JwksUri(String),
    /// The key set itself. Nothing is fetched.
    Inline(Value),
}

impl IssuerKeys {
    /// How a stored row says where its keys are. `None` for a row whose
    /// columns disagree with its `key_source`, which the migration's check
    /// does not allow and which verifies nothing.
    pub fn of(row: &oidc_issuers::Model) -> Option<Self> {
        match row.key_source.as_str() {
            KEY_SOURCE_DISCOVERY => Some(Self::Discovery {
                discovery_url: row.discovery_url.clone(),
                jwks_uri: row.jwks_uri.clone()?,
            }),
            KEY_SOURCE_JWKS_URI => row.jwks_uri.clone().map(Self::JwksUri),
            KEY_SOURCE_INLINE => row.jwks.clone().map(Self::Inline),
            _ => None,
        }
    }

    /// The URL keys are fetched from, if any.
    pub fn jwks_uri(&self) -> Option<&str> {
        match self {
            Self::Discovery { jwks_uri, .. } | Self::JwksUri(jwks_uri) => Some(jwks_uri),
            Self::Inline(_) => None,
        }
    }
}

/// What to register: its key source already read and checked.
#[derive(Clone, Debug)]
pub struct NewIssuer {
    pub org_id: Uuid,
    pub name: String,
    /// Without a trailing `/`.
    pub issuer: String,
    pub keys: IssuerKeys,
    pub jti_single_use: bool,
    /// Which settings-form preset filled it in; display only.
    pub preset: Option<String>,
    /// The person registering it — audit only.
    pub created_by: Uuid,
}

/// Why a registration was not stored.
#[derive(Debug)]
pub enum CreateError {
    /// The org already registered this issuer.
    Duplicate,
    Db(OxyError),
}

/// Register the issuer. The org's `(org_id, issuer)` is unique.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    new: NewIssuer,
) -> Result<oidc_issuers::Model, CreateError> {
    let (key_source, discovery_url, jwks_uri, jwks) = match new.keys {
        IssuerKeys::Discovery {
            discovery_url,
            jwks_uri,
        } => (KEY_SOURCE_DISCOVERY, discovery_url, Some(jwks_uri), None),
        IssuerKeys::JwksUri(jwks_uri) => (KEY_SOURCE_JWKS_URI, None, Some(jwks_uri), None),
        IssuerKeys::Inline(jwks) => (KEY_SOURCE_INLINE, None, None, Some(jwks)),
    };
    let row = oidc_issuers::ActiveModel {
        id: Set(Uuid::new_v4()),
        org_id: Set(new.org_id),
        name: Set(new.name),
        issuer: Set(normalized_issuer(&new.issuer).to_string()),
        key_source: Set(key_source.to_string()),
        discovery_url: Set(discovery_url),
        jwks_uri: Set(jwks_uri),
        jwks: Set(jwks),
        jti_single_use: Set(new.jti_single_use),
        preset: Set(new.preset),
        created_by: Set(Some(new.created_by)),
        created_at: Set(Utc::now().fixed_offset()),
    };
    match row.insert(db).await {
        Ok(row) => Ok(row),
        Err(e) => match e.sql_err() {
            Some(SqlErr::UniqueConstraintViolation(_)) => Err(CreateError::Duplicate),
            _ => Err(CreateError::Db(db_err("create oidc issuer")(e))),
        },
    }
}

/// The org's issuers, oldest first.
pub async fn list<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
) -> Result<Vec<oidc_issuers::Model>, OxyError> {
    OidcIssuers::find()
        .filter(oidc_issuers::Column::OrgId.eq(org_id))
        .order_by_asc(oidc_issuers::Column::CreatedAt)
        .order_by_asc(oidc_issuers::Column::Id)
        .all(db)
        .await
        .map_err(db_err("list oidc issuers"))
}

/// One issuer **of this org**. Any other reads the same as none.
pub async fn find<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    issuer_id: Uuid,
) -> Result<Option<oidc_issuers::Model>, OxyError> {
    Ok(OidcIssuers::find_by_id(issuer_id)
        .one(db)
        .await
        .map_err(db_err("oidc issuer lookup"))?
        .filter(|i| i.org_id == org_id))
}

/// The org's issuer whose `issuer` is `iss`, a trailing `/` ignored.
pub async fn find_by_issuer<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    iss: &str,
) -> Result<Option<oidc_issuers::Model>, OxyError> {
    OidcIssuers::find()
        .filter(oidc_issuers::Column::OrgId.eq(org_id))
        .filter(oidc_issuers::Column::Issuer.eq(normalized_issuer(iss)))
        .one(db)
        .await
        .map_err(db_err("oidc issuer by iss"))
}

/// Take the issuer's row lock **of this org**, and hand back the row as it
/// is now. What deleting it does first: a mint in flight from one of its
/// policies holds it shared ([`super::issuer_trust_mint`]), so that mint's
/// token is committed, and revoked by the delete, before the delete goes on.
#[must_use = "decide from the row this returns: it is the issuer as it is under the lock"]
pub async fn lock<C: ConnectionTrait>(
    db: &C,
    org_id: Uuid,
    issuer_id: Uuid,
) -> Result<Option<oidc_issuers::Model>, OxyError> {
    Ok(OidcIssuers::find_by_id(issuer_id)
        .lock_exclusive()
        .one(db)
        .await
        .map_err(db_err("lock oidc issuer"))?
        .filter(|i| i.org_id == org_id))
}

/// Delete the issuer. Its policies, and their grants, go with it
/// (`ON DELETE CASCADE`); the caller revokes what they minted first.
pub async fn delete<C: ConnectionTrait>(db: &C, issuer_id: Uuid) -> Result<(), OxyError> {
    OidcIssuers::delete_by_id(issuer_id)
        .exec(db)
        .await
        .map_err(db_err("delete oidc issuer"))?;
    Ok(())
}
