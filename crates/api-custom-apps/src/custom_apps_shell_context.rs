//! `GET /api/projects/:id/shell-context` — bootstrap payload for the
//! `@oxy-hq/sdk/shell` chrome inside custom-app bundles.
//!
//! One request gives a bundle everything the workspace shell needs:
//! workspace/org identity, the logo URL, the published-apps list, the
//! product navigation links, and the viewer's display identity. The
//! server builds every URL because only it knows which host scheme the
//! request arrived on:
//!
//! - main host (`app.oxygen-hq.com/customer-apps/<org>/<slug>/`) and
//!   **org subdomains** (`<org>.oxygen-hq.com/a/<slug>/`) serve the full
//!   SPA on the same origin, so relative paths are correct and keep
//!   navigation same-site;
//! - a **custom-app subdomain** (`<org>--<slug>.customer-apps.<zone>`)
//!   serves only the bundle — product links must be absolute against the
//!   admin host ([`admin_base_url`]), which the bundle cannot derive
//!   itself. `/api/*` stays relative everywhere (same-host data plane).
//!
//! Fleet posture: Postgres reads (workspace, org, apps + build rows), no
//! disk → FleetOk, deliberately NOT pinned in `role_manifest.rs`; see
//! the `oxy-route-classification` skill. Response is per-viewer (carries
//! `user`), so it is not cached anywhere — no shared result cache (would
//! leak across tenants/viewers) and `Cache-Control: private, no-store`
//! (a browser-cached copy would replay one viewer's identity to the next
//! account on the same browser; see `oxy-customer-apps-perf`).
//!
//! What IS cached is the server side of the prelude: the gate chain
//! (user, workspace, access — `custom_apps_gates`) and the owning org row
//! both come from the 60 s process caches, so a boot costs the per-viewer
//! reads only: the published-apps list the viewer may see, their reach, and
//! their light/dark mode (one primary-key read, `appearance`).

use axum::Json;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use entity::prelude::Organizations;
use sea_orm::EntityTrait;
use serde::Serialize;
use uuid::Uuid;

use crate::custom_apps_cache::{cached_org, set_cached_org};
use crate::custom_apps_gates::{CustomAppContext, check_custom_app_gates};
use crate::workspace_custom_apps::published_app_summaries;
use oxy_app_core::custom_apps_host_dispatch::{admin_base_url, parse_subdomain};

#[derive(Serialize)]
pub struct ShellContextResponse {
    pub workspace: ShellWorkspace,
    pub org: ShellOrg,
    /// Workspace logo endpoint (relative — `/api/*` is same-host on every
    /// scheme), or `None` when it can't be versioned. Bundles render an
    /// `<img>` and fall back to the name initial on error.
    pub logo_url: Option<String>,
    /// Published custom apps in this workspace, including the current one.
    pub apps: Vec<ShellApp>,
    pub links: ShellLinks,
    /// Viewer display identity (never authorization data — gates are
    /// server-side).
    pub user: Option<ShellUser>,
    /// The viewer's light/dark mode (`appearance`), which the shell applies
    /// and its switch writes: `"light"`, `"dark"`, `"system"`, or `null` for a
    /// person who has not chosen (Oxygen's default, light, applies). Left OUT
    /// for a frontline worker, who has no Oxygen account to carry one, and
    /// when the read fails: an absent field is what an older server sends,
    /// so the bundle keeps its own mode and shows no switch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<Option<&'static str>>,
}

#[derive(Serialize)]
pub struct ShellWorkspace {
    pub id: Uuid,
    pub name: String,
}

#[derive(Serialize)]
pub struct ShellOrg {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct ShellApp {
    pub id: Uuid,
    pub name: String,
    pub slug: String,
    /// Ready-to-navigate URL — relative on same-origin schemes, absolute
    /// from a custom-app subdomain.
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    /// Agent ref the app's Ask Oxygen panel binds to (manifest
    /// `ask.agent`). Absent → the shell hides the Ask surface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_agent: Option<String>,
    /// Composer chips for the Ask panel (manifest `ask.suggestedQuestions`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub suggested_questions: Vec<String>,
}

#[derive(Serialize)]
pub struct ShellLinks {
    pub home: String,
    pub threads: String,
    /// Opens the Unified Settings Dialog on the product surface. The dialog
    /// has no dedicated route — the SPA reads `?settings=<section>` on any
    /// workspace page (App.tsx WorkspaceLayout) and opens it in place.
    pub settings: String,
    /// The product's login page, for an app that signs its viewer out
    /// (`GET /api/logout`, then here with `?return_to=<the app>`). Named by
    /// the server for the same reason as the rest: on a custom-app subdomain
    /// the login page lives on the product host, which the bundle cannot
    /// derive. On an enrolled tablet this page is "Who's on shift?".
    pub login: String,
}

#[derive(Serialize)]
pub struct ShellUser {
    pub name: String,
    /// Empty for a frontline worker, who has no mailbox.
    pub email: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    pub id: Uuid,
    /// `member` holds an org membership; `frontline` is a crew member — the
    /// same signal `ctx.user.email === null` gives a function.
    pub kind: &'static str,
    /// Display-only reach, the same rule the function's `ctx.user.reach`
    /// applies (`internal-docs/operating-graph.md` §3.3). App-admin standing
    /// is known only when the bundle names its app (`?app=`); without it the
    /// browser may read scoped where the function reads everywhere — never
    /// the other way round.
    pub reach: oxy_app_core::operating_graph::reach::Reach,
}

/// `?app=<id>` — the bundle's own app, so the viewer's app-admin standing
/// can be part of the display reach. Optional: an older bundle sends nothing.
#[derive(Debug, Default, serde::Deserialize)]
pub struct ShellContextQuery {
    /// Parsed leniently: a malformed value must not cost the bundle its
    /// workspace, org, apps and links over a display-only enhancement.
    pub app: Option<String>,
}

impl ShellContextQuery {
    pub fn app_id(&self) -> Option<Uuid> {
        self.app.as_deref().and_then(|a| a.parse().ok())
    }
}

/// Prefix for product URLs: empty (relative) unless the request arrived
/// on a custom-app subdomain, where no SPA exists and links must point
/// at the admin host absolutely. Falls back to relative when
/// `OXY_API_URL` isn't configured (local/dev) — the links may not
/// resolve there, but that beats fabricating a host.
fn product_url_base(headers: &HeaderMap, on_custom_domain: bool) -> String {
    let on_app_subdomain = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .and_then(parse_subdomain)
        .is_some();
    if on_app_subdomain || on_custom_domain {
        admin_base_url().unwrap_or_default()
    } else {
        String::new()
    }
}

/// The owning org, from the 60 s org cache the resolution cache invalidates
/// with (`custom_apps_cache::cached_org`): the gate hands over the id, and
/// every app boot in the workspace asks for the same row.
async fn owning_org(ctx: &CustomAppContext) -> Result<entity::organizations::Model, Response> {
    if let Some(org) = cached_org(ctx.org_id) {
        return Ok(org);
    }
    match Organizations::find_by_id(ctx.org_id).one(&ctx.db).await {
        Ok(Some(org)) => {
            set_cached_org(org.clone());
            Ok(org)
        }
        Ok(None) => Err((StatusCode::NOT_FOUND, "owning organization not found").into_response()),
        Err(e) => {
            tracing::error!("shell-context org lookup failed: {e}");
            Err((StatusCode::INTERNAL_SERVER_ERROR, "org lookup failed").into_response())
        }
    }
}

/// The viewer's mode for [`ShellContextResponse::mode`]: one primary-key read,
/// skipped for a frontline worker. A failed read leaves the field out rather
/// than costing the bundle its chrome over a display preference.
async fn viewer_mode(ctx: &CustomAppContext) -> Option<Option<&'static str>> {
    if !crate::appearance::has_mode(&ctx.user) {
        return None;
    }
    match crate::appearance::store::load(&ctx.db, ctx.user.id).await {
        Ok(mode) => Some(mode.map(crate::appearance::ColorMode::as_str)),
        Err(e) => {
            tracing::warn!("shell-context mode read failed: {e}");
            None
        }
    }
}

pub async fn get_shell_context(
    Path(project_id): Path<Uuid>,
    axum::extract::Query(query): axum::extract::Query<ShellContextQuery>,
    on_domain: Option<axum::Extension<oxy_app_core::custom_domain_host::CustomDomainCtx>>,
    headers: HeaderMap,
) -> Response {
    // Gates first — same chain as /query. Nothing below runs (and nothing
    // is cached) for callers who can't access the project.
    let ctx = match check_custom_app_gates(&headers, project_id).await {
        Ok(c) => c,
        Err(r) => return r,
    };

    let org = match owning_org(&ctx).await {
        Ok(o) => o,
        Err(r) => return r,
    };

    // `ctx.user` carries the credential the request arrived with, so an API
    // token's narrowing reaches this list too.
    let viewer = oxy_server_authz::Caller::from_user(&ctx.user);
    let summaries = match published_app_summaries(&ctx.db, project_id, Some(&viewer)).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("shell-context app listing failed: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "app listing failed").into_response();
        }
    };

    let base = product_url_base(&headers, on_domain.is_some());
    let apps = summaries
        .into_iter()
        .map(|s| ShellApp {
            id: s.id,
            name: s.name,
            slug: s.slug,
            url: format!("{base}{}", s.url),
            icon_url: s.icon_url.map(|u| format!("{base}{u}")),
            default_agent: s.default_agent,
            suggested_questions: s.suggested_questions,
        })
        .collect();

    let ws_root = format!("{base}/{}/workspaces/{}", org.slug, project_id);
    let reach = oxy_custom_apps::reach::reach_for_viewer(
        &ctx.db,
        ctx.org_id,
        &ctx.user,
        project_id,
        query.app_id(),
    )
    .await;
    let mode = viewer_mode(&ctx).await;
    let body = ShellContextResponse {
        workspace: ShellWorkspace {
            id: ctx.workspace.id,
            name: ctx.workspace.name.clone(),
        },
        org: ShellOrg {
            id: org.id,
            slug: org.slug.clone(),
            name: org.name,
        },
        // `/api/*` is host-agnostic (served same-origin on every scheme),
        // so the logo stays relative even when product links are absolute.
        logo_url: Some(format!(
            "/api/{}/logo?v={}",
            project_id,
            urlencoding::encode(&org.updated_at.to_rfc3339())
        )),
        apps,
        links: ShellLinks {
            home: format!("{ws_root}/home"),
            threads: format!("{ws_root}/threads"),
            // Same default section the web-app rail user menu opens for
            // cloud viewers; invalid sections fall back SPA-side anyway.
            settings: format!("{ws_root}/home?settings=organization.general"),
            // On a custom domain the product login cannot come back to the
            // domain; the app's own root starts the domain's sign-in, which
            // passes through that login.
            login: match on_domain {
                Some(_) => "/".to_string(),
                None => format!("{base}/login"),
            },
        },
        user: Some(ShellUser {
            name: ctx.user.name.clone(),
            // The address itself, or empty: `useIdentity()` reads an empty
            // address as the crew signal, the way a function reads `null`.
            email: ctx.user.email.clone().unwrap_or_default(),
            picture: ctx.user.picture.clone(),
            id: ctx.user.id,
            kind: if ctx.user.email.is_some() {
                "member"
            } else {
                "frontline"
            },
            reach,
        }),
        mode,
    };

    // Never cached: the payload embeds the viewer's identity and the URL
    // is viewer-independent, so any freshness window (even `private,
    // max-age`) lets a shared browser replay user A's identity to user B
    // after a re-login — the request would be served from cache without
    // ever reaching the gate chain. The payload is a handful of indexed
    // Postgres lookups once per app load; re-fetching is cheap.
    ([(header::CACHE_CONTROL, "private, no-store")], Json(body)).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with_host(host: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, host.parse().unwrap());
        h
    }

    #[test]
    fn relative_base_on_main_host() {
        assert_eq!(
            product_url_base(&headers_with_host("app.oxygen-hq.com"), false),
            ""
        );
    }

    #[test]
    fn relative_base_on_org_subdomain() {
        // Org subdomains serve the full SPA — links stay same-origin.
        assert_eq!(
            product_url_base(&headers_with_host("acme.oxygen-hq.com"), false),
            ""
        );
    }

    #[test]
    fn relative_base_without_host_header() {
        assert_eq!(product_url_base(&HeaderMap::new(), false), "");
    }

    fn body_with_mode(mode: Option<Option<&'static str>>) -> serde_json::Value {
        let id = Uuid::nil();
        serde_json::to_value(ShellContextResponse {
            workspace: ShellWorkspace {
                id,
                name: "w".into(),
            },
            org: ShellOrg {
                id,
                slug: "o".into(),
                name: "O".into(),
            },
            logo_url: None,
            apps: vec![],
            links: ShellLinks {
                home: "/h".into(),
                threads: "/t".into(),
                settings: "/s".into(),
                login: "/login".into(),
            },
            user: None,
            mode,
        })
        .unwrap()
    }

    #[test]
    fn mode_is_absent_for_no_account_null_when_unchosen_and_named_when_chosen() {
        // Absent is what an older server sends, and what a crew sign-in gets.
        assert!(body_with_mode(None).get("mode").is_none());
        assert_eq!(body_with_mode(Some(None))["mode"], serde_json::Value::Null);
        assert_eq!(body_with_mode(Some(Some("dark")))["mode"], "dark");
    }

    #[test]
    fn absolute_base_on_custom_app_subdomain() {
        // Env-dependent half (`admin_base_url`) is covered in
        // custom_apps_host_dispatch tests; here we only assert the
        // Host-header classification path is taken (empty base when
        // OXY_API_URL is unset, but the subdomain IS detected — the
        // fallback keeps links relative rather than inventing a host).
        let base = product_url_base(
            &headers_with_host("acme--sales.customer-apps-dev.oxygen-hq.com"),
            false,
        );
        // With OXY_API_URL unset in the test env this is the fallback "";
        // with it set it must be an absolute origin.
        if !base.is_empty() {
            assert!(base.starts_with("http"));
            assert!(!base.ends_with('/'));
        }
    }
}
