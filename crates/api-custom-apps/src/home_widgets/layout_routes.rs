//! The org's home layout, as `oxyc` and agents read and write it:
//!
//! - `GET /api/orgs/{org_id}/home` — what the home looks like (`describe`).
//! - `PUT /api/orgs/{org_id}/home/layout` — `{ layout, revision?, dry_run? }`.
//! - `DELETE /api/orgs/{org_id}/home/layout` — back to the default.
//!
//! All three are the org's owners and admins (`OrgAdmin`), staff acting as one
//! included: the layout is the org's, so it is written and described against
//! every published app in the org, not one person's view of it. What a person
//! sees on their home is still only the apps they can open (`GET /home/widgets`).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{Router, get, put};
use oxy::database::client::establish_connection;
use oxy_app::surface::role_guards::OrgAdmin;
use oxy_app_core::AppState;
use oxy_app_core::widgets::layout::{CatalogEntry, check, read};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::DatabaseConnection;
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::instrument;
use uuid::Uuid;

use super::describe::{OrgRef, describe};
use super::layout_store::{self, WriteError};
use super::load::{self, AppWidgets};

/// Mounted under `/orgs/{org_id}/home`, behind `org_middleware`.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/", get(get_home))
        .route("/layout", put(put_layout).delete(reset_layout))
}

/// `GET /api/orgs/{org_id}/home` — the sections the home draws in order, the
/// widgets in each at their size, every widget the org's apps declare, and the
/// layout behind it with its revision. `effective_layout` is the round-trip form.
#[instrument(skip_all, fields(org = %ctx.org.slug))]
pub async fn get_home(OrgAdmin(ctx): OrgAdmin) -> Response {
    let org = org_ref(&ctx.org);
    let Ok(db) = establish_connection().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let (apps, stored) = match org_state(&db, org.id).await {
        Ok(state) => state,
        Err(status) => return status.into_response(),
    };
    let catalog = load::catalog(&apps);
    let layout = stored.as_ref().and_then(|s| read(&s.layout));
    let stored = stored.map(|s| (s.layout, s.revision));
    Json(describe(org, stored, layout.as_ref(), &catalog)).into_response()
}

#[derive(Debug, Deserialize)]
pub struct PutBody {
    pub layout: Value,
    /// The revision the writer read; a stale one is refused. Absent: written
    /// over whatever is there. Present but `null` is refused (`Some(None)`): a
    /// client's broken number must never turn into an unchecked overwrite.
    #[serde(default, deserialize_with = "present")]
    pub revision: Option<Option<i64>>,
    /// Describe the home this layout would draw, saving nothing.
    #[serde(default)]
    pub dry_run: bool,
}

/// `PUT /api/orgs/{org_id}/home/layout`
#[instrument(skip_all, fields(org = %ctx.org.slug))]
pub async fn put_layout(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Json(body): Json<PutBody>,
) -> Response {
    let org = org_ref(&ctx.org);
    let Ok(db) = establish_connection().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let (apps, stored) = match org_state(&db, org.id).await {
        Ok(state) => state,
        Err(status) => return status.into_response(),
    };
    let Some(expected) = expected_revision(body.revision) else {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({
                "error": "revision_invalid",
                "message": "`revision` is the integer GET …/home returned; leave it out only to overwrite",
            })),
        )
            .into_response();
    };
    let catalog = load::catalog(&apps);
    let entries: Vec<CatalogEntry> = catalog.iter().map(|(e, _)| e.clone()).collect();
    let layout = match check(&body.layout, &entries) {
        Ok(layout) => layout,
        Err(problems) => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(json!({ "error": "layout_invalid", "problems": problems })),
            )
                .into_response();
        }
    };
    if body.dry_run {
        let revision = stored.as_ref().map(|s| s.revision).unwrap_or(0);
        let preview = describe(org, Some((body.layout, revision)), Some(&layout), &catalog);
        return Json(with_dry_run(preview)).into_response();
    }
    let revision = match layout_store::save(&db, org.id, &body.layout, expected, user.id).await {
        Ok(revision) => revision,
        Err(WriteError::Stale { current }) => return stale(expected, current),
        Err(WriteError::Db(e)) => return internal(e).into_response(),
    };
    tracing::info!(org = %org.slug, revision, by = %user.id, "home layout written");
    Json(describe(
        org,
        Some((body.layout, revision)),
        Some(&layout),
        &catalog,
    ))
    .into_response()
}

/// `DELETE /api/orgs/{org_id}/home/layout` — the default home again.
#[instrument(skip_all, fields(org = %ctx.org.slug))]
pub async fn reset_layout(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
) -> Response {
    let org = org_ref(&ctx.org);
    let Ok(db) = establish_connection().await else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let cleared = match layout_store::reset(&db, org.id).await {
        Ok(cleared) => cleared,
        Err(e) => return internal(e).into_response(),
    };
    if cleared {
        tracing::info!(org = %org.slug, by = %user.id, "home layout reset");
    }
    let apps = match org_widget_apps(&db, org.id).await {
        Ok(apps) => apps,
        Err(status) => return status.into_response(),
    };
    Json(describe(org, None, None, &load::catalog(&apps))).into_response()
}

/// Every published app in the org with its widgets, and the stored layout.
async fn org_state(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<(Vec<AppWidgets>, Option<layout_store::Stored>), StatusCode> {
    let apps = org_widget_apps(db, org_id).await?;
    let stored = layout_store::load(db, org_id).await.map_err(internal)?;
    Ok((apps, stored))
}

async fn org_widget_apps(
    db: &DatabaseConnection,
    org_id: Uuid,
) -> Result<Vec<AppWidgets>, StatusCode> {
    let rows = load::org_apps(db, org_id).await.map_err(internal)?;
    load::with_widgets(db, rows).await.map_err(internal)
}

fn org_ref(org: &entity::organizations::Model) -> OrgRef {
    OrgRef {
        id: org.id,
        slug: org.slug.clone(),
        name: org.name.clone(),
    }
}

/// A field that is present — `null` included — is `Some`.
fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Option<i64>>, D::Error> {
    Option::<i64>::deserialize(d).map(Some)
}

/// The revision to check against: `Some(None)` when there is none to check
/// (absent), `None` when it was sent as `null` and must be refused.
pub fn expected_revision(sent: Option<Option<i64>>) -> Option<Option<i64>> {
    match sent {
        None => Some(None),
        Some(Some(revision)) => Some(Some(revision)),
        Some(None) => None,
    }
}

fn with_dry_run(description: super::describe::Description) -> Value {
    let mut value = serde_json::to_value(description).unwrap_or(Value::Null);
    if let Some(obj) = value.as_object_mut() {
        obj.insert("dry_run".into(), Value::Bool(true));
    }
    value
}

fn stale(sent: Option<i64>, current: i64) -> Response {
    (
        StatusCode::CONFLICT,
        Json(json!({
            "error": "layout_changed",
            "message": format!(
                "the layout is at revision {current}, not {}; read it again and reapply your change",
                sent.unwrap_or(0)
            ),
            "revision": current,
        })),
    )
        .into_response()
}

fn internal(e: sea_orm::DbErr) -> StatusCode {
    tracing::error!("home layout: database call failed: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}
