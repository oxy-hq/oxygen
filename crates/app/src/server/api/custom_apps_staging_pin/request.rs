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
//! the request is a non-production request, and the caller may open
//! non-production. Such a request is one addressed to a **non-production
//! environment** of the app — its staging host, a sandbox's host, or either
//! named by `X-Oxy-App-Env` on a bearer request (the caller must pass
//! `may_open_non_production`, the rule that serves them that environment's
//! HTML) — or one carrying the **preview cookie** (the caller must have
//! platform `DevelopApps` reach for the app's org — the decision
//! `custom_apps_serve` makes to serve the draft bundle). Any miss is "no pin":
//! the request reads the promoted revision, as it did before staging existed.
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
/// every other request. Cheap for a production request with no preview cookie
/// (no DB work).
///
/// Two entrances, each with its own reach rule and its own build:
///
/// - **a non-production environment** — the staging host
///   (`staging--<org>--<slug>.…`, environments design §3.2), a sandbox's host
///   (`dev-<handle>--<org>--<slug>.…`), or either named by `X-Oxy-App-Env` on
///   a bearer or API-key request: the viewer must be allowed to open
///   non-production (`may_open_non_production`, the rule that serves them that
///   environment's HTML), and the pin is read from the build **that
///   environment** serves. A sandbox whose build pins nothing reads the
///   promoted model; it never borrows staging's pin;
/// - **the preview cookie** on a production request: `DevelopApps` reach, and
///   the app's draft build — which the staging row mirrors, so the same build.
pub async fn staging_pin_for_data_request(
    db: &DatabaseConnection,
    headers: &HeaderMap,
    user_id: Uuid,
    user_email: &str,
    project_id: Uuid,
) -> Option<Uuid> {
    let non_production = oxy_app_core::custom_app_env_request::request_environment(headers)
        .ok()
        .filter(|environment| *environment != AppEnvironment::Production);
    if non_production.is_none()
        && !crate::server::api::custom_apps_preview::wants_draft_preview(headers)
    {
        return None;
    }
    let app = find_app(db, &app_ref(headers)?).await?;
    if app.project_id != project_id {
        return None;
    }
    let build_id = match non_production {
        Some(environment) => environment_build(db, user_id, user_email, &app, &environment).await?,
        None => draft_preview_build(db, user_email, &app).await?,
    };
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

/// The draft build, for a caller with `DevelopApps` reach over the app's org —
/// the decision `custom_apps_serve` makes to serve the draft bundle.
async fn draft_preview_build(
    db: &DatabaseConnection,
    user_email: &str,
    app: &entity::apps::Model,
) -> Option<Uuid> {
    let reaches = oxy_server_authz::globals::platform_reaches(
        db,
        user_email,
        oxy_authz::Cap::DevelopApps,
        app.org_id,
    )
    .await;
    if !reaches {
        return None;
    }
    app.draft_build_id
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

async fn find_app(db: &DatabaseConnection, r: &AppRef) -> Option<entity::apps::Model> {
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
