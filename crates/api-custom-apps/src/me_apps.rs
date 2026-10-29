//! `GET /api/me/apps` — every published custom app the caller can open, across
//! every org they belong to. The mobile host app's home screen.
//!
//! Flat, not under `/{workspace_id}` or `/orgs/{org_id}`: nothing in the path
//! names a tenant, so no middleware has gated one, and every app is decided
//! here by `oxy-authz` — the `AppAccess` ring, asked once per row against
//! facts loaded once per request. That ring already admits a frontline worker
//! to an app they hold an explicit grant on, so PIN identities get their apps
//! through the same door rather than an exemption.
//!
//! The cards are the workspace launcher's (`workspace_custom_apps`): the same
//! manifest-derived description, icon and art, the same `?v=` build pin. What
//! differs is that a native client has no origin to resolve a relative URL
//! against, so every URL here is absolute.

use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use entity::apps;
use entity::prelude::Apps;
use oxy::database::client::establish_connection;
use oxy_app_core::custom_app_environment::AppEnvironment;
use oxy_app_core::custom_apps_host_dispatch::environment_url_for;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_authz::PrincipalFacts;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde::Serialize;
use std::collections::BTreeSet;
use uuid::Uuid;

use crate::workspace_custom_apps::{LauncherApp, app_access_resource, launcher_apps};
use oxy_app::surface::session::extract_link_base_for_authenticated_request;

#[derive(Debug, Serialize)]
pub struct MyAppsResponse {
    pub apps: Vec<MyApp>,
}

/// One app card. Optional fields serialize as `null`, never absent, so a
/// client can decode a fixed shape.
#[derive(Debug, Serialize)]
pub struct MyApp {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub org: MyAppOrg,
    pub workspace_id: Uuid,
    /// Where the app opens: its own host when the deployment has a
    /// custom-apps zone, else the path route on the host the client called.
    pub url: String,
    pub icon_url: Option<String>,
    pub art_url: Option<String>,
    pub published_at: String,
}

#[derive(Debug, Serialize)]
pub struct MyAppOrg {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

/// Every published custom app the caller can open, across all their orgs,
/// sorted by org name then app name. Each app is decided by the `AppAccess`
/// ring, so a restricted app appears only with a grant, a frontline worker
/// sees only the apps granted to them, and an API token lists only what it
/// reaches.
pub async fn list_openable_apps(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    headers: HeaderMap,
) -> Result<Json<MyAppsResponse>, StatusCode> {
    let db = establish_connection().await.map_err(|e| {
        tracing::error!("me/apps DB connect failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    // The user AND the credential the request arrived with, so a token's
    // grants and blocked orgs narrow the list as they narrow every answer.
    let caller = oxy_server_authz::Caller::from_user(&user);
    // Unknown is not empty: an empty list on a database blip would tell the
    // client this person has no apps. 503 says "ask again".
    let facts = oxy_server_authz::loader::load_principal_facts_scoped(&db, &caller, false)
        .await
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let base = extract_link_base_for_authenticated_request(&headers);
    let apps = openable_apps(&db, &facts, &base).await.map_err(|e| {
        tracing::error!("me/apps listing failed: {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(MyAppsResponse { apps }))
}

/// The cards for `facts`, with path URLs made absolute against `base`. Three
/// queries whatever the number of orgs or workspaces: the apps, their orgs,
/// their manifests.
pub async fn openable_apps(
    db: &sea_orm::DatabaseConnection,
    facts: &PrincipalFacts,
    base: &str,
) -> Result<Vec<MyApp>, sea_orm::DbErr> {
    let orgs = candidate_orgs(facts);
    if orgs.is_empty() {
        return Ok(vec![]);
    }
    let rows = Apps::find()
        .filter(apps::Column::OrgId.is_in(orgs))
        .filter(apps::Column::PublishedAt.is_not_null())
        .all(db)
        .await?;
    let rows = keep_openable(facts, rows);
    let mut out: Vec<MyApp> = launcher_apps(db, rows)
        .await?
        .into_iter()
        .map(|app| {
            let host = environment_url_for(
                &AppEnvironment::Production,
                &app.summary.org_slug,
                &app.summary.slug,
            );
            MyApp::from_launcher(app, host, base)
        })
        .collect();
    sort_for_display(&mut out);
    Ok(out)
}

/// The orgs whose apps are worth asking the ring about: the ones the caller
/// BELONGS to — a membership, a frontline enrolment, or the service account's
/// own org. Staff and partner reach is deliberately not here: "my apps" is not
/// every tenant a Global Admin or a distributor could open.
pub(crate) fn candidate_orgs(facts: &PrincipalFacts) -> BTreeSet<Uuid> {
    facts
        .member_orgs
        .iter()
        .chain(&facts.frontline_orgs)
        .copied()
        .chain(facts.service_account.map(|account| account.org_id))
        .collect()
}

/// The rows `facts` may open. Every row goes through the ring — unrestricted
/// ones too, unlike the workspace list, because no workspace middleware has
/// established membership first. The resource names the workspace the app was
/// published from, so a workspace-scoped token is capped there rather than by
/// its best grant anywhere in the org.
pub(crate) fn keep_openable(facts: &PrincipalFacts, rows: Vec<apps::Model>) -> Vec<apps::Model> {
    rows.into_iter()
        .filter(|app| {
            oxy_authz::allows(
                facts,
                oxy_authz::Action::AppAccess,
                &app_access_resource(app).published_from(app.project_id),
            )
        })
        .collect()
}

/// `path` (root-relative) on `base` (scheme + host, any trailing `/`).
pub(crate) fn absolute(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

impl MyApp {
    /// `app_host` is the app's production host when the deployment has one.
    /// Icon and art stay on `base` either way: it is the host the client
    /// already holds a credential for, and the path route accepts a bearer.
    fn from_launcher(app: LauncherApp, app_host: Option<String>, base: &str) -> Self {
        let LauncherApp {
            org_id,
            org_name,
            workspace_id,
            summary,
        } = app;
        Self {
            url: app_host.unwrap_or_else(|| absolute(base, &summary.url)),
            icon_url: summary.icon_url.map(|u| absolute(base, &u)),
            art_url: summary.art_url.map(|u| absolute(base, &u)),
            id: summary.id,
            slug: summary.slug,
            name: summary.name,
            description: summary.description,
            org: MyAppOrg {
                id: org_id,
                slug: summary.org_slug,
                name: org_name,
            },
            workspace_id,
            published_at: summary.published_at,
        }
    }
}

/// Org name, then app name, ignoring case; ids break ties so two orgs that
/// share a name never interleave and the order is stable across requests.
fn sort_for_display(apps: &mut [MyApp]) {
    apps.sort_by_cached_key(|a| {
        (
            a.org.name.to_lowercase(),
            a.org.id,
            a.name.to_lowercase(),
            a.id,
        )
    });
}

#[cfg(test)]
#[path = "me_apps_tests.rs"]
mod tests;
