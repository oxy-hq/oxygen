//! HTTP surface for feature flags.
//!
//! - Admin (`router`, mounted at `/admin/feature-flags` behind the staff surface's
//!   `PlatformOperate`): list, patch a flag's kill switch and rules, and preview
//!   what any org or person would get.
//! - Org member (`org_flags`, mounted at `/orgs/{org_id}/feature-flags`): the
//!   evaluated variant of every `client` flag for the caller. Values only, never
//!   the rules: an org must not learn which other orgs are named in an override.

use std::collections::BTreeMap;

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use chrono::{DateTime, Utc};
use oxy_app_core::audit::{AuditEntry, RequestActor, record_best_effort};
use sea_orm::DatabaseConnection;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use super::evaluate::{Evaluation, FlagContext};
use super::registry::{self, FlagDef, Unit};
use super::targeting::Targeting;
use super::{cache, store};
use crate::server::api::middlewares::org_context::OrgContextExtractor;
use crate::server::router::AppState;

/// The audit action of a PATCH.
pub const UPDATED: &str = "admin.feature_flag.updated";

#[derive(Serialize)]
pub struct FeatureFlagDto {
    pub key: &'static str,
    pub description: &'static str,
    pub default: bool,
    /// The kill switch.
    pub enabled: bool,
    pub updated_at: Option<DateTime<Utc>>,
    pub variants: &'static [&'static str],
    pub unit: Unit,
    pub per_org: bool,
    pub client: bool,
    /// `None` for a plain global flag.
    pub targeting: Option<Targeting>,
    /// Why the stored rules are not served, when they are not.
    pub targeting_error: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateBody {
    /// The kill switch. Absent leaves it.
    #[serde(default)]
    pub enabled: Option<bool>,
    /// Absent leaves the rules; `null` clears them (a plain global flag again).
    #[serde(default, with = "::serde_with::rust::double_option")]
    pub targeting: Option<Option<Targeting>>,
}

#[derive(Deserialize)]
pub struct EvaluateQuery {
    pub org_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
}

#[derive(Serialize)]
pub struct EvaluationDto {
    pub key: &'static str,
    #[serde(flatten)]
    pub evaluation: Evaluation,
}

#[derive(Serialize)]
pub struct ClientFlag {
    pub variant: &'static str,
    /// Anything but the off/control variant.
    pub enabled: bool,
}

#[derive(Serialize)]
pub struct OrgFlagsResponse {
    pub flags: BTreeMap<&'static str, ClientFlag>,
}

pub(crate) fn router() -> Router<AppState> {
    Router::new()
        .route("/feature-flags", get(list))
        .route("/feature-flags/evaluate", get(evaluate))
        .route("/feature-flags/{key}", patch(update))
}

fn internal(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> Response {
    move |e| {
        tracing::error!(error = %e, "feature_flags {what} failed");
        StatusCode::INTERNAL_SERVER_ERROR.into_response()
    }
}

fn unprocessable(message: String) -> Response {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(json!({ "error": message })),
    )
        .into_response()
}

async fn connect() -> Result<DatabaseConnection, Response> {
    oxy::database::client::establish_connection()
        .await
        .map_err(|e| {
            tracing::error!(?e, "feature_flags: db connect failed");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        })
}

fn dto(def: &'static FlagDef, row: Option<&entity::feature_flag::Model>) -> FeatureFlagDto {
    let decoded = row.map(store::decode);
    let (enabled, targeting, targeting_error) = match decoded {
        None => (def.default_enabled, None, None),
        Some(d) => match d.targeting {
            Ok(t) => {
                let error = t.as_ref().and_then(|t| t.compile(def).err());
                (d.enabled, t, error)
            }
            Err(e) => (d.enabled, None, Some(e)),
        },
    };
    FeatureFlagDto {
        key: def.key,
        description: def.description,
        default: def.default_enabled,
        enabled,
        updated_at: row.map(|r| r.updated_at.with_timezone(&Utc)),
        variants: def.variants,
        unit: def.unit,
        per_org: def.per_org,
        client: def.client,
        targeting,
        targeting_error,
    }
}

async fn list() -> Result<Json<Vec<FeatureFlagDto>>, Response> {
    let db = connect().await?;
    let rows = store::fetch_all(&db).await.map_err(internal("list"))?;
    let out = registry::FLAGS
        .iter()
        .map(|def| dto(def, rows.iter().find(|r| r.key == def.key)))
        .collect();
    Ok(Json(out))
}

/// What `body` makes the flag, from its `current` row. The error is the response.
fn next_state(
    def: &FlagDef,
    current: Option<&store::Decoded>,
    body: UpdateBody,
) -> Result<(bool, Option<Targeting>), Response> {
    let enabled = body
        .enabled
        .or(current.map(|c| c.enabled))
        .unwrap_or(def.default_enabled);
    let targeting = match body.targeting {
        Some(next) => next,
        None => match current.map(|c| c.targeting.clone()) {
            None => None,
            Some(Ok(t)) => t,
            Some(Err(reason)) => {
                let message = format!("{reason}; send `targeting` to replace or clear it");
                return Err(
                    (StatusCode::CONFLICT, Json(json!({ "error": message }))).into_response()
                );
            }
        },
    };
    if let Some(t) = &targeting {
        if !def.per_org {
            return Err(unprocessable(format!(
                "{} is read without an org, so rules would do nothing; it takes only `enabled`",
                def.key
            )));
        }
        t.compile(def).map_err(unprocessable)?;
    }
    Ok((enabled, targeting))
}

async fn update(
    actor: RequestActor,
    Path(key): Path<String>,
    Json(body): Json<UpdateBody>,
) -> Result<Json<FeatureFlagDto>, Response> {
    let Some(def) = registry::get(&key) else {
        return Err((StatusCode::NOT_FOUND, "unknown feature flag").into_response());
    };
    let db = connect().await?;
    let before_row = store::fetch(&db, def.key).await.map_err(internal("read"))?;
    let before = dto(def, before_row.as_ref());
    let decoded = before_row.as_ref().map(store::decode);
    let (enabled, targeting) = next_state(def, decoded.as_ref(), body)?;

    let row = store::write(&db, def, enabled, targeting.as_ref())
        .await
        .map_err(internal("write"))?;
    cache::set_state(def.key, cache::state_of(def, &store::decode(&row)));

    let after = dto(def, Some(&row));
    let entry = AuditEntry::for_request(&actor, UPDATED)
        .target("feature_flag", def.key, def.key)
        .change(audit_view(&before), audit_view(&after));
    record_best_effort(&db, entry).await;
    Ok(Json(after))
}

fn audit_view(flag: &FeatureFlagDto) -> serde_json::Value {
    json!({ "enabled": flag.enabled, "targeting": flag.targeting })
}

/// What every flag serves the given org and person on THIS instance, with the
/// reason. A preview, so it records no exposure.
async fn evaluate(Query(q): Query<EvaluateQuery>) -> Json<Vec<EvaluationDto>> {
    let ctx = FlagContext {
        org_id: q.org_id,
        user_id: q.user_id,
    };
    let out = registry::FLAGS
        .iter()
        .filter_map(|def| {
            cache::evaluate(def.key, &ctx).map(|evaluation| EvaluationDto {
                key: def.key,
                evaluation,
            })
        })
        .collect();
    Json(out)
}

/// `GET /orgs/{org_id}/feature-flags`: every `client` flag's variant for the
/// caller in this org. This is the web app's exposure point for a split, so it
/// records one.
pub async fn org_flags(OrgContextExtractor(ctx): OrgContextExtractor) -> Json<OrgFlagsResponse> {
    let context = FlagContext::member(ctx.org.id, ctx.membership.user_id);
    let flags = registry::FLAGS
        .iter()
        .filter(|def| def.client)
        .map(|def| {
            let eval = super::evaluate_for(def.key, &context);
            let flag = ClientFlag {
                variant: eval.variant,
                enabled: eval.variant != def.off(),
            };
            (def.key, flag)
        })
        .collect();
    Json(OrgFlagsResponse { flags })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::feature_flags::registry::BOOLEAN;

    const GLOBAL: FlagDef = FlagDef {
        key: "test_global",
        description: "",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        per_org: false,
        client: false,
    };
    const PER_ORG: FlagDef = FlagDef {
        per_org: true,
        ..GLOBAL
    };

    fn body(json: serde_json::Value) -> UpdateBody {
        serde_json::from_value(json).unwrap()
    }

    fn rules() -> serde_json::Value {
        json!({ "fallthrough": { "type": "split", "weights": { "on": 1000, "off": 9000 } } })
    }

    #[test]
    fn the_legacy_body_still_just_flips_the_switch() {
        let (enabled, targeting) =
            next_state(&GLOBAL, None, body(json!({ "enabled": true }))).unwrap();
        assert!(enabled);
        assert!(targeting.is_none());
    }

    #[test]
    fn rules_on_a_flag_read_without_an_org_are_refused() {
        let err = next_state(&GLOBAL, None, body(json!({ "targeting": rules() }))).unwrap_err();
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(next_state(&PER_ORG, None, body(json!({ "targeting": rules() }))).is_ok());
    }

    #[test]
    fn absent_keeps_the_rules_and_null_clears_them() {
        let current = store::Decoded {
            enabled: true,
            targeting: Ok(Some(serde_json::from_value(rules()).unwrap())),
        };
        let (enabled, kept) =
            next_state(&PER_ORG, Some(&current), body(json!({ "enabled": false }))).unwrap();
        assert!(!enabled);
        assert!(kept.is_some());
        let (enabled, cleared) =
            next_state(&PER_ORG, Some(&current), body(json!({ "targeting": null }))).unwrap();
        assert!(enabled);
        assert!(cleared.is_none());
    }

    #[test]
    fn invalid_rules_are_refused_with_the_reason() {
        let bad =
            json!({ "targeting": { "fallthrough": { "type": "variant", "variant": "beta" } } });
        let err = next_state(&PER_ORG, None, body(bad)).unwrap_err();
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[test]
    fn unreadable_stored_rules_must_be_replaced_not_kept() {
        let current = store::Decoded {
            enabled: true,
            targeting: Err("stored targeting is unreadable".into()),
        };
        let err =
            next_state(&PER_ORG, Some(&current), body(json!({ "enabled": false }))).unwrap_err();
        assert_eq!(err.status(), StatusCode::CONFLICT);
        assert!(next_state(&PER_ORG, Some(&current), body(json!({ "targeting": null }))).is_ok());
    }
}
