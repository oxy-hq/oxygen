//! The token issuers an org trusts: list, register, delete (API-tokens design
//! §3.4, *Any OIDC issuer*). The handlers are in [`super::handlers`]; this is
//! what they do.
//!
//! **Registering** stores the issuer — its `iss`, without a trailing `/` — and
//! where its keys come from, one of:
//!
//! - `discovery` (the default): its OIDC discovery document is read from
//!   `<discovery_url or issuer>/.well-known/openid-configuration`, must name
//!   this issuer, and must keep its `jwks_uri` on the host it was read from;
//!   that `jwks_uri` is stored;
//! - `jwks_uri`: a key-set URL the admin gives;
//! - `inline`: the key set itself, pasted. Nothing is fetched, so the issuer
//!   may be one Oxygen cannot reach (one on a private network).
//!
//! Every fetch goes through [`crate::oidc_issuer_fetch`] — public HTTPS only,
//! no redirects — and the keys must hold an RSA signing key with a `kid`. The
//! exchange reads keys from what was stored and nowhere else.
//!
//! `jti_single_use` (on unless asked off) spends each token's `jti` at the
//! exchange. `preset` records which settings-form preset filled the
//! registration in; nothing reads it but the screen.
//!
//! **Deleting** an issuer ends every trust policy on it, in every account of
//! the org, and revokes the tokens they minted — in one transaction that first
//! takes the issuer's row lock, which a mint in flight from one of its
//! policies holds shared.

use std::collections::{HashMap, HashSet};

use entity::prelude::Users;
use entity::{api_tokens, oidc_issuers, users};
use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use oxy_app_core::audit::{self, AuditEntry, RequestActor};
use oxy_auth::github_oidc::GITHUB_OIDC_ISSUER;
use oxy_auth::token::access::Invalid;
use oxy_auth::token::oidc_issuer::{self, CreateError, IssuerKeys, NewIssuer};
use oxy_auth::token::{ci, issuer_trust_policy};
use oxy_auth::workload_identity::{keys, normalized_issuer};
use oxy_shared::http::public_only;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, QueryFilter, TransactionTrait,
};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

use super::dto::{CreatorDto, OidcIssuerDto};
use super::trust_policies::invalidate;
use crate::oidc_issuer_fetch::fetch_json;
use crate::user_tokens::error::TokenError;

const TARGET_TYPE: &str = "oidc_issuer";
const REGISTERED: &str = "oidc_issuer.registered";
const DELETED: &str = "oidc_issuer.deleted";

/// The longest name an issuer may be given.
pub const NAME_MAX_CHARS: usize = 64;
/// The longest issuer, discovery URL or key-set URL.
pub const URL_MAX_CHARS: usize = 512;
/// The most an inline key set may be, as JSON.
pub const INLINE_JWKS_MAX_BYTES: usize = 64 * 1024;
/// The longest preset name.
pub const PRESET_MAX_CHARS: usize = 32;

const DISCOVERY_PATH: &str = "/.well-known/openid-configuration";

/// `POST /api/orgs/{org_id}/oidc-issuers`.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct CreateIssuerBody {
    pub name: String,
    pub issuer: String,
    /// `discovery` (when left out), `jwks_uri` or `inline`.
    pub key_source: Option<String>,
    /// `discovery` only: where to read the document, when not the issuer.
    pub discovery_url: Option<String>,
    /// `jwks_uri` only.
    pub jwks_uri: Option<String>,
    /// `inline` only: a JSON Web Key Set, public keys only.
    pub jwks: Option<Value>,
    /// On when left out.
    pub jti_single_use: Option<bool>,
    /// Which settings-form preset filled it in: `aws`, `custom`.
    pub preset: Option<String>,
}

/// Where the keys will come from, checked for shape; nothing fetched yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KeysWant {
    Discovery { discovery_url: Option<String> },
    JwksUri(String),
    Inline(Value),
}

/// A registration, checked for shape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IssuerWant {
    pub name: String,
    /// Without a trailing `/`.
    pub issuer: String,
    pub keys: KeysWant,
    pub jti_single_use: bool,
    pub preset: Option<String>,
}

fn invalid(message: impl Into<String>) -> Invalid {
    Invalid(message.into())
}

impl CreateIssuerBody {
    pub fn want(&self) -> Result<IssuerWant, Invalid> {
        let source = self
            .key_source
            .as_deref()
            .unwrap_or(oidc_issuers::KEY_SOURCE_DISCOVERY);
        let keys = match source {
            oidc_issuers::KEY_SOURCE_DISCOVERY => {
                if self.jwks_uri.is_some() || self.jwks.is_some() {
                    return Err(invalid(
                        "keys read through discovery take no 'jwks_uri' or 'jwks'",
                    ));
                }
                KeysWant::Discovery {
                    discovery_url: self
                        .discovery_url
                        .as_deref()
                        .map(|raw| clean_url("discovery_url", raw))
                        .transpose()?,
                }
            }
            oidc_issuers::KEY_SOURCE_JWKS_URI => {
                if self.discovery_url.is_some() || self.jwks.is_some() {
                    return Err(invalid(
                        "keys read from a 'jwks_uri' take no 'discovery_url' or 'jwks'",
                    ));
                }
                let raw = self
                    .jwks_uri
                    .as_deref()
                    .ok_or_else(|| invalid("give the 'jwks_uri' the issuer's keys are at"))?;
                KeysWant::JwksUri(clean_url("jwks_uri", raw)?)
            }
            oidc_issuers::KEY_SOURCE_INLINE => {
                if self.discovery_url.is_some() || self.jwks_uri.is_some() {
                    return Err(invalid(
                        "inline keys take no 'discovery_url' or 'jwks_uri': nothing is fetched",
                    ));
                }
                let jwks = self
                    .jwks
                    .as_ref()
                    .ok_or_else(|| invalid("paste the issuer's public keys as 'jwks'"))?;
                check_inline_keys(jwks).map_err(invalid)?;
                KeysWant::Inline(jwks.clone())
            }
            _ => {
                return Err(invalid(
                    "'key_source' must be 'discovery', 'jwks_uri' or 'inline'",
                ));
            }
        };
        // The issuer is fetched from only when its own discovery document is
        // the key source; otherwise it may be an address Oxygen cannot reach.
        let fetched = matches!(
            keys,
            KeysWant::Discovery {
                discovery_url: None
            }
        );
        Ok(IssuerWant {
            name: clean_name(&self.name)?,
            issuer: clean_issuer(&self.issuer, fetched)?,
            keys,
            jti_single_use: self.jti_single_use.unwrap_or(true),
            preset: self.preset.as_deref().map(clean_preset).transpose()?,
        })
    }
}

/// What the org calls the issuer: `oxy-prod`.
pub fn clean_name(raw: &str) -> Result<String, Invalid> {
    let name = raw.trim();
    let shaped = !name.is_empty()
        && name.chars().count() <= NAME_MAX_CHARS
        && !name.chars().any(char::is_control);
    if shaped {
        Ok(name.to_string())
    } else {
        Err(invalid(format!(
            "'name' must be 1 to {NAME_MAX_CHARS} printable characters"
        )))
    }
}

/// An https URL with a host, and no credentials, query or fragment.
fn plain_https(raw: &str) -> Option<Url> {
    if raw.is_empty() || raw.chars().count() > URL_MAX_CHARS {
        return None;
    }
    let url = Url::parse(raw).ok()?;
    let shaped = url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none();
    shaped.then_some(url)
}

/// The issuer, as its tokens carry it in `iss`, without a trailing `/`. A
/// public host only when Oxygen reads its discovery document from it
/// (`fetched`); with keys from elsewhere it may be private.
pub fn clean_issuer(raw: &str, fetched: bool) -> Result<String, Invalid> {
    let issuer = normalized_issuer(raw.trim());
    let bad = || {
        invalid(
            "'issuer' must be the issuer URL exactly as its tokens carry it in 'iss': https, no \
             query or fragment",
        )
    };
    let url = plain_https(issuer).ok_or_else(bad)?;
    if fetched && !public_only::is_safe_outbound(&url) {
        return Err(invalid(
            "the issuer is not a public HTTPS address, so its discovery document cannot be read: \
             give a public 'discovery_url' or 'jwks_uri', or paste its keys inline",
        ));
    }
    if issuer == GITHUB_OIDC_ISSUER {
        return Err(invalid(
            "GitHub Actions is trusted through a GitHub trust policy on the service account, \
             not as an issuer",
        ));
    }
    Ok(issuer.to_string())
}

/// A URL Oxygen fetches: https, a public host, no credentials, query or
/// fragment.
pub fn clean_url(field: &str, raw: &str) -> Result<String, Invalid> {
    let raw = raw.trim();
    match plain_https(raw) {
        Some(url) if public_only::is_safe_outbound(&url) => Ok(raw.to_string()),
        _ => Err(invalid(format!(
            "'{field}' must be a public https URL with no query or fragment"
        ))),
    }
}

/// A preset name: lower-case letters, digits and `-`.
pub fn clean_preset(raw: &str) -> Result<String, Invalid> {
    let preset = raw.trim();
    let shaped = !preset.is_empty()
        && preset.len() <= PRESET_MAX_CHARS
        && preset
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if shaped {
        Ok(preset.to_string())
    } else {
        Err(invalid(format!(
            "'preset' must be 1 to {PRESET_MAX_CHARS} lower-case letters, digits and '-'"
        )))
    }
}

/// Where a discovery document is: OIDC Discovery §4, the base with any
/// trailing `/` removed, then the well-known path — unless the base already
/// is the document's address.
pub fn discovery_url(base: &str) -> String {
    let base = base.trim_end_matches('/');
    if base.ends_with(DISCOVERY_PATH) {
        base.to_string()
    } else {
        format!("{base}{DISCOVERY_PATH}")
    }
}

/// The host and port of a URL, as compared for "the same host".
fn authority(url: &Url) -> Option<(String, Option<u16>)> {
    Some((
        url.host_str()?.to_ascii_lowercase(),
        url.port_or_known_default(),
    ))
}

/// The `jwks_uri` a discovery document read from `base` names, if the
/// document describes `issuer`: its `issuer` is that (a trailing `/`
/// ignored), and its keys are on `base`'s host, over HTTPS. Same host because
/// the address an admin typed is the one whose owner they trust; a document
/// that sends key reads elsewhere is refused rather than followed — give its
/// `jwks_uri` directly instead.
pub fn jwks_uri_of(issuer: &str, base: &str, document: &Value) -> Result<String, String> {
    let named = document.get("issuer").and_then(Value::as_str);
    if named.map(normalized_issuer) != Some(normalized_issuer(issuer)) {
        return Err(format!(
            "the discovery document at {} names issuer {}, not {issuer}: register the issuer \
             exactly as its tokens carry it",
            discovery_url(base),
            named.unwrap_or("none"),
        ));
    }
    let jwks_uri = document
        .get("jwks_uri")
        .and_then(Value::as_str)
        .ok_or_else(|| "the discovery document names no jwks_uri".to_string())?;
    let base_url = Url::parse(base).map_err(|e| e.to_string())?;
    let keys_url =
        Url::parse(jwks_uri).map_err(|_| format!("its jwks_uri {jwks_uri} is not a URL"))?;
    let same_host = keys_url.scheme() == "https" && authority(&keys_url) == authority(&base_url);
    if !same_host {
        return Err(format!(
            "its jwks_uri {jwks_uri} is not on {} over https: register the issuer with that \
             'jwks_uri' instead",
            base_url.host_str().unwrap_or(base),
        ));
    }
    Ok(jwks_uri.to_string())
}

fn key_set(keys: &Value) -> Result<JwkSet, String> {
    serde_json::from_value(keys.clone()).map_err(|_| "not a JSON Web Key Set".to_string())
}

/// Whether a key set holds at least one RSA key with a `kid` a token could
/// name.
pub fn has_signing_key(keys: &Value) -> Result<(), String> {
    let set = key_set(keys).map_err(|e| format!("the issuer's keys are {e}"))?;
    let usable = set.keys.iter().any(|key| {
        matches!(key.algorithm, AlgorithmParameters::RSA(_)) && key.common.key_id.is_some()
    });
    if usable {
        Ok(())
    } else {
        Err("the issuer publishes no RSA signing key with a 'kid'".into())
    }
}

/// An inline key set: public keys only, small, with a usable RSA key.
pub fn check_inline_keys(jwks: &Value) -> Result<(), String> {
    if jwks.to_string().len() > INLINE_JWKS_MAX_BYTES {
        return Err(format!("'jwks' is more than {INLINE_JWKS_MAX_BYTES} bytes"));
    }
    let private = jwks
        .get("keys")
        .and_then(Value::as_array)
        .is_some_and(|keys| keys.iter().any(|k| k.get("d").is_some()));
    if private {
        return Err("'jwks' holds a private key: paste the public keys only".into());
    }
    has_signing_key(jwks)
}

/// The `kid`s of an inline key set, as the screen shows it.
pub fn inline_key_ids(jwks: &Value) -> Vec<String> {
    key_set(jwks)
        .map(|set| {
            set.keys
                .into_iter()
                .filter_map(|key| key.common.key_id)
                .collect()
        })
        .unwrap_or_default()
}

/// Read what the key source needs read, and say where keys will come from.
async fn resolve(issuer: &str, keys: KeysWant) -> Result<IssuerKeys, TokenError> {
    match keys {
        KeysWant::Discovery { discovery_url: url } => {
            let base = url.as_deref().unwrap_or(issuer);
            let document = fetch_json(&discovery_url(base))
                .await
                .map_err(TokenError::IssuerUnusable)?;
            let jwks_uri =
                jwks_uri_of(issuer, base, &document).map_err(TokenError::IssuerUnusable)?;
            read_keys(&jwks_uri).await?;
            Ok(IssuerKeys::Discovery {
                discovery_url: url,
                jwks_uri,
            })
        }
        KeysWant::JwksUri(jwks_uri) => {
            read_keys(&jwks_uri).await?;
            Ok(IssuerKeys::JwksUri(jwks_uri))
        }
        KeysWant::Inline(jwks) => Ok(IssuerKeys::Inline(jwks)),
    }
}

async fn read_keys(jwks_uri: &str) -> Result<(), TokenError> {
    let keys = fetch_json(jwks_uri)
        .await
        .map_err(TokenError::IssuerUnusable)?;
    has_signing_key(&keys).map_err(TokenError::IssuerUnusable)
}

/// Who each of `ids` is, as a creator is shown.
pub(super) async fn creators_by_id<C: ConnectionTrait>(
    db: &C,
    ids: &[Uuid],
) -> Result<HashMap<Uuid, CreatorDto>, TokenError> {
    let ids: HashSet<Uuid> = ids.iter().copied().collect();
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let found = Users::find()
        .filter(users::Column::Id.is_in(ids))
        .all(db)
        .await?;
    Ok(found
        .into_iter()
        .map(|u| {
            let label = u.label().to_string();
            (u.id, CreatorDto { id: u.id, label })
        })
        .collect())
}

async fn dtos<C: ConnectionTrait>(
    db: &C,
    rows: &[oidc_issuers::Model],
) -> Result<Vec<OidcIssuerDto>, TokenError> {
    let ids: Vec<Uuid> = rows.iter().map(|r| r.id).collect();
    let counts = issuer_trust_policy::counts_by_issuer(db, &ids).await?;
    let creator_ids: Vec<Uuid> = rows.iter().filter_map(|r| r.created_by).collect();
    let creators = creators_by_id(db, &creator_ids).await?;
    Ok(rows
        .iter()
        .map(|row| {
            let creator = row.created_by.and_then(|id| creators.get(&id).cloned());
            let count = counts.get(&row.id).copied().unwrap_or(0);
            let key_ids = row.jwks.as_ref().map(inline_key_ids).unwrap_or_default();
            OidcIssuerDto::of(row, key_ids, count, creator)
        })
        .collect())
}

pub(super) async fn list(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<Vec<OidcIssuerDto>, TokenError> {
    let rows = oidc_issuer::list(db, org_id).await?;
    dtos(db, &rows).await
}

fn summary(row: &oidc_issuers::Model) -> Value {
    json!({
        "name": row.name,
        "issuer": row.issuer,
        "key_source": row.key_source,
        "discovery_url": row.discovery_url,
        "jwks_uri": row.jwks_uri,
        "inline_key_ids": row.jwks.as_ref().map(inline_key_ids),
        "jti_single_use": row.jti_single_use,
        "preset": row.preset,
    })
}

fn entry(actor: &RequestActor, action: &'static str, row: &oidc_issuers::Model) -> AuditEntry {
    AuditEntry::for_request(actor, action)
        .org(row.org_id)
        .target(TARGET_TYPE, row.id.to_string(), row.name.clone())
}

pub(super) async fn create(
    db: &DatabaseConnection,
    actor: &RequestActor,
    org_id: Uuid,
    body: CreateIssuerBody,
) -> Result<OidcIssuerDto, TokenError> {
    let want = body.want()?;
    // Before reading anything from the issuer: a second registration of the
    // same one is refused without a request to it. The unique key refuses a
    // race between two.
    if oidc_issuer::find_by_issuer(db, org_id, &want.issuer)
        .await?
        .is_some()
    {
        return Err(TokenError::IssuerExists);
    }
    let keys = resolve(&want.issuer, want.keys).await?;
    let new = NewIssuer {
        org_id,
        name: want.name,
        issuer: want.issuer,
        keys,
        jti_single_use: want.jti_single_use,
        preset: want.preset,
        created_by: actor.id,
    };
    let txn = db.begin().await?;
    let row = match oidc_issuer::create(&txn, new).await {
        Ok(row) => row,
        Err(CreateError::Duplicate) => return Err(TokenError::IssuerExists),
        Err(CreateError::Db(e)) => return Err(e.into()),
    };
    let created = entry(actor, REGISTERED, &row).metadata(summary(&row));
    audit::record_in_txn(&txn, created).await?;
    txn.commit().await?;
    let mut out = dtos(db, std::slice::from_ref(&row)).await?;
    out.pop()
        .ok_or_else(|| TokenError::Internal("an issuer mapped to nothing".into()))
}

/// Delete the issuer as it is under its lock: revoke what its policies
/// minted, unlink those tokens, and delete it with its policies.
pub(super) async fn delete(
    db: &DatabaseConnection,
    actor: &RequestActor,
    org_id: Uuid,
    issuer_id: Uuid,
) -> Result<(), TokenError> {
    let txn = db.begin().await?;
    let row = oidc_issuer::lock(&txn, org_id, issuer_id)
        .await?
        .ok_or(TokenError::NotFound)?;
    let policies = issuer_trust_policy::on_issuer(&txn, issuer_id).await?;
    let mut revoked: Vec<api_tokens::Model> = Vec::new();
    for policy in &policies {
        revoked.extend(ci::revoke_for_policy(&txn, policy.id, actor.id).await?);
    }
    let policy_ids: Vec<Uuid> = policies.iter().map(|p| p.id).collect();
    issuer_trust_policy::unlink_tokens(&txn, &policy_ids).await?;
    oidc_issuer::delete(&txn, issuer_id).await?;
    let detail = json!({
        "deleted_policies": policies.len(),
        "revoked_tokens": revoked.len(),
    });
    let deleted = entry(actor, DELETED, &row)
        .metadata(detail)
        .change(summary(&row), json!(null));
    audit::record_in_txn(&txn, deleted).await?;
    txn.commit().await?;
    invalidate(&revoked);
    if let Some(jwks_uri) = &row.jwks_uri {
        keys::forget(jwks_uri);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EKS: &str = "https://oidc.eks.us-west-2.amazonaws.com/id/EXAMPLED539D4633E53DE1B71";

    fn rsa_keys() -> Value {
        json!({ "keys": [{
            "kty": "RSA", "kid": "k", "use": "sig", "alg": "RS256",
            "n": "sXchDaQebHnPiGvyDOAT4saGEUetSyo9MKLOoWFsueri23bOdgWp4Dy1WlUzewbgBHod5pcM9H95GQRV3JDXboIRROSBigeC5yjU1hGzHHyXss8UDprecbAYxknTcQkhslANGRUZmdTOQ5qTRsLAt6BTYuyvVRdhS8exSZEy_c4gs_7svlJJQ4H9_NxsiIoLwAEk7-Q3UXERGYw_75IDrGA84-lA_-Ct4eTlXHBIY2EaV7t7LjJaynVJCpkv4LKjTTAumiGUIuQhrNhZLuF_RJLqHpM2kgWFLU7-VTdL1VbC2tejvcI2BlMkEpk1BzBZI0KQB0GaDWFLN-aEAw3vRw",
            "e": "AQAB"
        }]})
    }

    fn body() -> CreateIssuerBody {
        CreateIssuerBody {
            name: "oxy-prod".into(),
            issuer: EKS.into(),
            ..Default::default()
        }
    }

    #[test]
    fn an_issuer_is_https_and_kept_without_a_trailing_slash() {
        assert_eq!(
            clean_issuer(&format!(" {EKS} "), true).ok().as_deref(),
            Some(EKS)
        );
        assert_eq!(
            clean_issuer(&format!("{EKS}/"), true).ok().as_deref(),
            Some(EKS)
        );
        for bad in [
            "",
            "oidc.eks.us-west-2.amazonaws.com/id/X",
            "http://oidc.eks.us-west-2.amazonaws.com/id/X",
            "https://user:pw@oidc.example.com",
            "https://oidc.example.com/?x=1",
            "https://oidc.example.com/#k",
        ] {
            assert!(clean_issuer(bad, false).is_err(), "{bad:?}");
        }
        let too_long = format!("https://example.com/{}", "a".repeat(URL_MAX_CHARS));
        assert!(clean_issuer(&too_long, false).is_err());
    }

    #[test]
    fn a_private_issuer_is_taken_only_when_nothing_is_fetched_from_it() {
        for private in [
            "https://kubernetes.default.svc",
            "https://kubernetes.default.svc.cluster.local",
            "https://10.0.0.5",
            "https://localhost",
        ] {
            assert!(clean_issuer(private, true).is_err(), "{private}");
            assert!(clean_issuer(private, false).is_ok(), "{private}");
        }
        let inline = CreateIssuerBody {
            issuer: "https://kubernetes.default.svc.cluster.local".into(),
            key_source: Some("inline".into()),
            jwks: Some(rsa_keys()),
            ..body()
        };
        assert!(inline.want().is_ok());
        let discovered = CreateIssuerBody {
            issuer: "https://kubernetes.default.svc.cluster.local".into(),
            ..body()
        };
        assert!(discovered.want().is_err());
    }

    #[test]
    fn github_actions_is_not_registered_as_an_issuer() {
        for github in [
            GITHUB_OIDC_ISSUER,
            "https://token.actions.githubusercontent.com/",
        ] {
            assert!(clean_issuer(github, true).is_err(), "{github}");
        }
    }

    #[test]
    fn each_key_source_takes_only_its_own_fields() {
        let want = body().want().expect("discovery by default");
        assert_eq!(
            want.keys,
            KeysWant::Discovery {
                discovery_url: None
            }
        );
        assert!(want.jti_single_use, "single use unless asked off");
        let jwks_uri = CreateIssuerBody {
            key_source: Some("jwks_uri".into()),
            jwks_uri: Some(format!("{EKS}/keys")),
            jti_single_use: Some(false),
            preset: Some("aws".into()),
            ..body()
        };
        let want = jwks_uri.want().expect("a jwks_uri");
        assert_eq!(want.keys, KeysWant::JwksUri(format!("{EKS}/keys")));
        assert!(!want.jti_single_use);
        assert_eq!(want.preset.as_deref(), Some("aws"));
        for bad in [
            CreateIssuerBody {
                key_source: Some("jwks_uri".into()),
                ..body()
            },
            CreateIssuerBody {
                key_source: Some("inline".into()),
                ..body()
            },
            CreateIssuerBody {
                jwks: Some(rsa_keys()),
                ..body()
            },
            CreateIssuerBody {
                key_source: Some("gitlab".into()),
                ..body()
            },
            CreateIssuerBody {
                key_source: Some("inline".into()),
                jwks: Some(rsa_keys()),
                jwks_uri: Some(format!("{EKS}/keys")),
                ..body()
            },
            CreateIssuerBody {
                key_source: Some("jwks_uri".into()),
                jwks_uri: Some("https://10.0.0.5/keys".into()),
                ..body()
            },
            CreateIssuerBody {
                preset: Some("AWS Account".into()),
                ..body()
            },
            CreateIssuerBody {
                name: "  ".into(),
                ..body()
            },
        ] {
            assert!(bad.want().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_discovery_document_is_read_from_the_well_known_path() {
        let doc = format!("{EKS}/.well-known/openid-configuration");
        assert_eq!(discovery_url(EKS), doc);
        assert_eq!(discovery_url(&format!("{EKS}/")), doc);
        assert_eq!(discovery_url(&doc), doc, "a full address is used as it is");
    }

    #[test]
    fn the_document_must_describe_this_issuer_and_keep_its_keys_at_home() {
        let keys = format!("{EKS}/keys");
        let good = json!({ "issuer": EKS, "jwks_uri": keys });
        assert_eq!(jwks_uri_of(EKS, EKS, &good).ok(), Some(keys.clone()));
        let slashed = json!({ "issuer": format!("{EKS}/"), "jwks_uri": keys });
        assert!(
            jwks_uri_of(EKS, EKS, &slashed).is_ok(),
            "a trailing slash is ignored"
        );
        // Read from a separate discovery URL: the keys must be on its host.
        let mirror = "https://discovery.example.com/eks";
        let mirrored = json!({ "issuer": EKS, "jwks_uri": "https://discovery.example.com/keys" });
        assert!(jwks_uri_of(EKS, mirror, &mirrored).is_ok());
        assert!(jwks_uri_of(EKS, mirror, &good).is_err());
        for document in [
            json!({ "issuer": "https://elsewhere.example", "jwks_uri": keys }),
            json!({ "jwks_uri": keys }),
            json!({ "issuer": EKS }),
            json!({ "issuer": EKS, "jwks_uri": "https://elsewhere.example/keys" }),
            json!({ "issuer": EKS, "jwks_uri": keys.replace("https://", "http://") }),
            json!({ "issuer": EKS, "jwks_uri": "not a url" }),
        ] {
            assert!(jwks_uri_of(EKS, EKS, &document).is_err(), "{document}");
        }
    }

    #[test]
    fn a_key_set_must_hold_an_rsa_key_with_a_kid() {
        assert!(has_signing_key(&rsa_keys()).is_ok());
        let mut no_kid = rsa_keys();
        no_kid["keys"][0].as_object_mut().unwrap().remove("kid");
        assert!(has_signing_key(&no_kid).is_err());
        let ec = json!({ "keys": [{
            "kty": "EC", "kid": "k", "crv": "P-256",
            "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0"
        }]});
        assert!(has_signing_key(&ec).is_err());
        assert!(has_signing_key(&json!({ "keys": [] })).is_err());
        assert!(has_signing_key(&json!({ "nope": 1 })).is_err());
    }

    #[test]
    fn inline_keys_are_public_and_small() {
        assert!(check_inline_keys(&rsa_keys()).is_ok());
        assert_eq!(inline_key_ids(&rsa_keys()), vec!["k".to_string()]);
        let mut private = rsa_keys();
        private["keys"][0]["d"] = json!("secret");
        assert!(check_inline_keys(&private).is_err());
        let mut big = rsa_keys();
        big["padding"] = json!("x".repeat(INLINE_JWKS_MAX_BYTES));
        assert!(check_inline_keys(&big).is_err());
    }
}
