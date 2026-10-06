//! Diagnostic snapshot for an installed custom app.
//!
//! `GET /api/customer-apps/{org_slug}/{app_slug}/debug` returns a structured
//! snapshot of everything an admin needs to diagnose why a custom app isn't
//! loading. Org-membership gated; response shape is for human inspection and
//! not guaranteed stable.
//!
//! Auth lives in `custom_apps_auth` and manifest resolution in
//! `custom_apps_manifest`; this module is a thin handler that assembles the
//! snapshot from those reusable helpers.

use axum::Json;
use axum::extract::Path;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::Value as JsonValue;
use tracing::instrument;
use uuid::Uuid;

use super::custom_apps_auth::{AuthOutcome, authenticate_and_authorize};
use super::custom_apps_manifest::{pick_channel_for, resolve_manifest};
use super::custom_apps_sync::Channel;

// ── Response types ───────────────────────────────────────────────────────────

/// Where the resolved manifest came from.
#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ManifestSource {
    /// `apps.manifest_override` is set; bundled `oxy-app.json` is ignored.
    DbOverride,
    /// No override; the `oxy-app.json` captured with the channel's build.
    BundleFile,
}

#[derive(Serialize)]
struct DebugSnapshot {
    org_slug: String,
    app_slug: String,
    app: AppSnapshot,
    /// The channel this request resolved to: `draft` on the app's staging host
    /// for a viewer who may open staging, or for an app never published;
    /// `published` otherwise.
    channel: &'static str,
    /// The build that channel points at (`app_builds.id`). `None` means the
    /// app has nothing to serve on this channel — the one bundle fault an
    /// operator can see from here.
    build: Option<Uuid>,
    manifest_source: ManifestSource,
    /// Raw parsed manifest JSON — useful for diagnostics without leaking
    /// admin-grade internal fields (project_id / branch are on the DB app row,
    /// not here).
    manifest: Option<JsonValue>,
    manifest_error: Option<String>,
}

#[derive(Serialize)]
struct AppSnapshot {
    id: Uuid,
    slug: String,
    name: String,
    status: String,
    source_type: String,
}

// ── Handler ──────────────────────────────────────────────────────────────────

/// `GET /api/customer-apps/{org_slug}/{app_slug}/debug`
#[instrument(skip_all, fields(org_slug = %org_slug, app_slug = %app_slug))]
pub async fn get_debug(
    Path((org_slug, app_slug)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let AuthOutcome { app, caller, .. } = match authenticate_and_authorize(
        &headers,
        &org_slug,
        &app_slug,
        oxy_auth::token::SandboxAgent::Refuse,
    )
    .await
    {
        Ok(v) => v,
        Err(status) => return status.into_response(),
    };
    let db = oxy::database::client::establish_connection().await;
    let on_staging = match &db {
        Ok(db) => on_staging_host(db, &headers, &caller, &app).await,
        Err(_) => false,
    };
    let channel = pick_channel_for(&app, on_staging);

    let mut snap = DebugSnapshot {
        org_slug,
        app_slug,
        app: AppSnapshot {
            id: app.id,
            slug: app.slug.clone(),
            name: app.name.clone(),
            status: app.status.clone(),
            source_type: app.source_type.clone(),
        },
        channel: channel.as_str(),
        build: match channel {
            Channel::Draft => app.draft_build_id,
            Channel::Published => app.published_build_id,
        },
        manifest_source: if app.manifest_override.is_some() {
            ManifestSource::DbOverride
        } else {
            ManifestSource::BundleFile
        },
        manifest: None,
        manifest_error: None,
    };

    let manifest = match &db {
        Ok(db) => resolve_manifest(db, &app, channel).await,
        Err(e) => Err(super::custom_apps_manifest::ManifestError::Io(
            e.to_string(),
        )),
    };
    match manifest {
        Ok(m) => {
            // Serialize the identity manifest back to a raw JSON value so the
            // debug consumer gets a stable opaque blob rather than a typed
            // struct that may drift from the on-disk schema.
            snap.manifest = serde_json::to_value(&m).ok();
        }
        Err(e) => {
            snap.manifest_error = Some(e.to_string());
        }
    }

    Json(snap).into_response()
}

/// Is this the app's staging host, opened by a viewer who may see staging? The
/// draft is the staging environment's, so the snapshot reports it only there —
/// the same decision that serves staging HTML. Fails closed: a lookup error is
/// "not staging".
async fn on_staging_host(
    db: &sea_orm::DatabaseConnection,
    headers: &HeaderMap,
    caller: &oxy_server_authz::Caller,
    app: &entity::apps::Model,
) -> bool {
    use oxy_app_core::custom_app_environment::AppEnvironment;
    let staging = matches!(
        oxy_app_core::custom_app_env_request::request_environment(headers),
        Ok(AppEnvironment::Staging)
    );
    staging && super::custom_apps_env_resolve::may_open_non_production(db, caller, app).await
}
