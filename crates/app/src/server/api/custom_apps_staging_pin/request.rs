//! Is a browser data-plane request a **non-production** request — staging's or
//! a sandbox's — and for which app?
//!
//! The SDK's data endpoints (`/api/projects/{project_id}/query`,
//! `/semantic-query`, the `semantic/*` analyses) are keyed by workspace, not
//! app — several apps can be published from one workspace. So the app is named
//! by the request itself, in this order:
//!
//! 1. the `x-oxy-app: <app uuid>` header the SDK's fetcher sends (SDK ≥ the
//!    release carrying this change);
//! 2. the `Referer` path — `/customer-apps/<org>/<slug>/…`, or `/a/<slug>/…`
//!    on an org subdomain — for bundles built with an older SDK;
//! 3. the `Host` of a custom-app subdomain (`<org>--<slug>.customer-apps.…`).
//!
//! Then the gates, all required: the app was published from THIS workspace,
//! the request is addressed to a **non-production environment** of the app —
//! its staging host, a sandbox's host, or either named by `X-Oxy-App-Env` on a
//! bearer request — and the caller may open non-production
//! (`may_open_non_production`, the rule that serves them that environment's
//! HTML). Any miss is "no pin": the request reads the promoted revision, as it
//! did before staging existed. The staff-only `oxy_preview_draft` cookie that
//! once made a production-host request a staging one is retired — a request
//! still carrying it reads the live revision like any other.
//!
//! **A spoofed header is not an authz hole.** It can only choose among apps of
//! the request's own workspace that the caller can already develop — whose
//! drafts that caller may preview anyway — and the gate chain
//! (`check_custom_app_gates`) has already decided the caller may read this
//! workspace's data at all.

use axum::http::HeaderMap;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use uuid::Uuid;

/// Header the SDK sends on data-plane calls to name the calling app.
pub const APP_HEADER: &str = "x-oxy-app";

/// How a request named its app, before any database lookup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AppRef {
    Id(Uuid),
    Slugs {
        org: String,
        app: String,
    },
    /// `/a/<slug>/` on an org subdomain: the org comes from the host.
    OrgHostSlug {
        org: String,
        app: String,
    },
}

/// The pin a non-production data-plane request should read, or `None` for
/// every other request. Cheap for a production request (no DB work).
///
/// A non-production request is one addressed to the staging host
/// (`staging--<org>--<slug>.…`, environments design §3.2), a sandbox's host
/// (`dev-<handle>--<org>--<slug>.…`), or either named by `X-Oxy-App-Env` on a
/// bearer or API-key request — never a cookie. The viewer must be allowed to
/// open non-production (`may_open_non_production`, the rule that serves them
/// that environment's HTML), and the pin is read from the build **that
/// environment** serves. A sandbox whose build pins nothing reads the promoted
/// model; it never borrows staging's pin.
pub async fn staging_pin_for_data_request(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    user_id: Uuid,
    user_email: &str,
    project_id: Uuid,
) -> Option<Uuid> {
    let non_production = oxy_app_core::custom_app_env_request::request_environment(headers)
        .ok()
        .filter(|environment| *environment != AppEnvironment::Production)?;
    let app = find_app(db, &app_ref(headers)?).await?;
    if app.project_id != project_id {
        return None;
    }
    let build_id = environment_build(db, user_id, user_email, &app, &non_production).await?;
    super::pinned_revision_for(db, build_id).await
}

/// The build `environment` — staging or a sandbox — serves, for a viewer who
/// may open the app's non-production environments.
async fn environment_build(
    db: &DatabaseConnection,
    user_id: Uuid,
    user_email: &str,
    app: &entity::apps::Model,
    environment: &AppEnvironment,
) -> Option<Uuid> {
    use crate::server::api::custom_apps_env_resolve::{
        may_open_non_production, resolve_environment,
    };
    if !may_open_non_production(db, user_id, user_email, app).await {
        return None;
    }
    resolve_environment(db, app, environment)
        .await
        .ok()?
        .build_id
}

/// Name the app from the request, in the documented order.
pub(crate) fn app_ref(headers: &HeaderMap) -> Option<AppRef> {
    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok());
    if let Some(id) = header(APP_HEADER).and_then(|v| Uuid::parse_str(v.trim()).ok()) {
        return Some(AppRef::Id(id));
    }
    if let Some(r) = header("referer").and_then(app_ref_from_referer) {
        return Some(r);
    }
    let host = header("host")?;
    let parsed = oxy_app_core::custom_apps_host_dispatch::parse_app_host(host)?;
    Some(AppRef::Slugs {
        org: parsed.org_slug,
        app: parsed.app_slug,
    })
}

/// The app a staging write names. The `Host` wins when it is an app host: a
/// staging page's `x-oxy-app` header cannot relabel its write as another
/// app's. Off an app host (an explicit `x-oxy-environment`), the documented
/// order. The agent ask, the automation run and the bundle's chat history all
/// name their app this way.
pub(crate) fn staging_app_ref(headers: &HeaderMap) -> Option<AppRef> {
    let host = headers
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(oxy_app_core::custom_apps_host_dispatch::parse_app_host);
    match host {
        Some(h) => Some(AppRef::Slugs {
            org: h.org_slug,
            app: h.app_slug,
        }),
        None => app_ref(headers),
    }
}

/// The app a staging request names ([`staging_app_ref`]), when it was
/// published from `project_id` and the caller may open its staging. `None` on
/// any miss, a lookup error included.
pub(crate) async fn staging_app_for(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    user_id: Uuid,
    user_email: &str,
    project_id: Uuid,
) -> Option<entity::apps::Model> {
    use crate::server::api::custom_apps_env_resolve::may_open_non_production;
    let app = find_app(db, &staging_app_ref(headers)?).await?;
    if app.project_id != project_id {
        return None;
    }
    may_open_non_production(db, user_id, user_email, &app)
        .await
        .then_some(app)
}

/// `404 EnvironmentRefused`: a surface outside production that this caller
/// may not reach. A 404, as for an app that does not exist, so it confirms
/// nothing. `what` names the surface (`"an agent ask"`, `"an automation
/// run"`) for the message.
pub(crate) fn environment_refused(
    environment: &AppEnvironment,
    what: &str,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    (
        axum::http::StatusCode::NOT_FOUND,
        axum::Json(serde_json::json!({
            "error": "EnvironmentRefused",
            "environment": environment.name(),
            "message": format!(
                "{what} in the {environment} environment runs only for a developer who \
                 may open this app's {environment}"
            ),
        })),
    )
        .into_response()
}

/// `https://host/customer-apps/<org>/<slug>/…` → slugs;
/// `https://<org>.<zone>/a/<slug>/…` → org-host slug;
/// `https://<org>--<slug>.customer-apps.<zone>/…` → slugs.
fn app_ref_from_referer(referer: &str) -> Option<AppRef> {
    let uri: axum::http::Uri = referer.parse().ok()?;
    let mut segs = uri.path().split('/').filter(|s| !s.is_empty());
    match (segs.next(), segs.next(), segs.next()) {
        (Some("customer-apps"), Some(org), Some(app)) => {
            return Some(AppRef::Slugs {
                org: org.to_string(),
                app: app.to_string(),
            });
        }
        (Some("a"), Some(app), _) => {
            let org = oxy_app_core::org_host_dispatch::parse_org_subdomain(uri.host()?)?;
            return Some(AppRef::OrgHostSlug {
                org,
                app: app.to_string(),
            });
        }
        _ => {}
    }
    let parsed = oxy_app_core::custom_apps_host_dispatch::parse_app_host(uri.host()?)?;
    Some(AppRef::Slugs {
        org: parsed.org_slug,
        app: parsed.app_slug,
    })
}

pub(crate) async fn find_app(db: &DatabaseConnection, r: &AppRef) -> Option<entity::apps::Model> {
    let found = match r {
        AppRef::Id(id) => entity::apps::Entity::find_by_id(*id).one(db).await,
        AppRef::Slugs { org, app } | AppRef::OrgHostSlug { org, app } => {
            let Ok(Some(org)) = entity::organizations::Entity::find()
                .filter(entity::organizations::Column::Slug.eq(org.as_str()))
                .one(db)
                .await
            else {
                return None;
            };
            entity::apps::Entity::find()
                .filter(entity::apps::Column::OrgId.eq(org.id))
                .filter(entity::apps::Column::Slug.eq(app.as_str()))
                .one(db)
                .await
        }
    };
    found.ok().flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn the_header_wins_over_referer_and_host() {
        let id = Uuid::new_v4();
        let h = headers(&[
            (APP_HEADER, &id.to_string()),
            (
                "referer",
                "https://app.example.com/customer-apps/acme/pulse/",
            ),
        ]);
        assert_eq!(app_ref(&h), Some(AppRef::Id(id)));
    }

    #[test]
    fn a_malformed_header_falls_back_to_the_referer_path() {
        let h = headers(&[
            (APP_HEADER, "not-a-uuid"),
            (
                "referer",
                "https://app.example.com/customer-apps/acme/pulse/orders?x=1",
            ),
        ]);
        assert_eq!(
            app_ref(&h),
            Some(AppRef::Slugs {
                org: "acme".into(),
                app: "pulse".into()
            })
        );
    }

    #[test]
    fn a_custom_app_subdomain_host_names_the_app() {
        let h = headers(&[("host", "acme--pulse.customer-apps.oxygen-hq.com")]);
        assert_eq!(
            app_ref(&h),
            Some(AppRef::Slugs {
                org: "acme".into(),
                app: "pulse".into()
            })
        );
    }

    #[test]
    fn an_unrelated_request_names_no_app() {
        let h = headers(&[
            ("host", "app.oxygen-hq.com"),
            ("referer", "https://app.oxygen-hq.com/ide/semantic"),
        ]);
        assert_eq!(app_ref(&h), None);
    }
}
