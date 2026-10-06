//! `?build=` naming a build only a **sandbox** serves is naming that sandbox.
//!
//! `invocation_reach` proves the rule for staging, whose build rides the
//! cached `EnvironmentBuilds`. A sandbox resolves from its own row instead
//! (`custom_apps_env_resolve::sandbox_row`), so the rule has to ask that row
//! too — otherwise a caller without reach naming a sandbox's build is
//! answered an empty list where staging's answers a refusal.
//!
//! The sandbox is created and published to for real, and its invocation is
//! written by a call on its `dev-<handle>--` host.

use axum::http::StatusCode;
use oxy_app::server::api::custom_apps_publish::{PublishTarget, publish_to};
use oxy_app::server::api::custom_apps_sandboxes::{TeardownReason, ops};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::json;
use uuid::Uuid;

use super::callers::{outsider, publish_token, staff};
use super::invocation_reach::{Listing, listed_as, of_build};
use super::readback::{APP, PRODUCTION_BUILD, STAGING_BUILD, build_pk, ids, ran, two_builds};
use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, seeded_tenant};
use crate::sandbox_publish::{app_row, bundle, input, sandbox};
use crate::staging_functions::{make_guest_staff, production_host};

const SANDBOX_BUILD: &str = "rb-dev-1";
const WHOAMI_JS: &str =
    "export default async (req, ctx) => Response.json({ channel: ctx.channel });";

fn sandbox_host(t: &Tenant, handle: &str) -> String {
    format!(
        "dev-{handle}--{}--{APP}.customer-apps.oxygen-hq.com",
        t.org_slug
    )
}

/// The sandbox `dev-<handle>` of the app, serving its own build `build`.
async fn sandbox_serving(t: &Tenant, app_id: Uuid, handle: &str, build: &str) {
    let app = app_row(&t.db, app_id).await;
    ops::create(&t.db, &app, &sandbox(handle), &t.guest())
        .await
        .expect("create the sandbox");
    let whoami = FunctionSpec {
        name: "whoami",
        manifest: json!({ "route": true }),
        js: WHOAMI_JS,
    };
    let tarball = bundle(APP, &[whoami], json!({}), &[]);
    publish_to(
        input(t, APP, build, tarball),
        PublishTarget::Sandbox(sandbox(handle)),
    )
    .await
    .expect("publish to the sandbox");
}

/// Naming the build a sandbox serves — by its publish id or its UUID — is
/// refused to a caller without reach and to a publish token, and lists the
/// build's rows to staff with reach. Naming production's build is not naming
/// a sandbox, even when a sandbox's pointer is on that build as well. Once
/// the sandbox is deleted no environment serves the build, and it is the
/// empty list for a caller without reach: its rows are still a sandbox's.
#[tokio::test]
async fn naming_a_build_only_a_sandbox_serves_needs_reach_and_productions_build_does_not() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    make_guest_staff();
    sandbox_serving(&t, app_id, "a1", SANDBOX_BUILD).await;
    // A second sandbox with no build: the rule reads every sandbox at once.
    let app = app_row(&t.db, app_id).await;
    ops::create(&t.db, &app, &sandbox("b2"), &t.guest())
        .await
        .expect("create a second sandbox");

    let live = ran(&t, APP, "whoami", &production_host(&t, APP))
        .await
        .to_string();
    let boxed = ran(&t, APP, "whoami", &sandbox_host(&t, "a1"))
        .await
        .to_string();
    let sandbox_uuid = build_pk(&t, app_id, SANDBOX_BUILD).await.to_string();

    for listing in [Listing::App, Listing::Function] {
        for build in [SANDBOX_BUILD, sandbox_uuid.as_str()] {
            let (status, body) =
                listed_as(listing, outsider(), None, app_id, of_build(build)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{listing:?} {build}: {body}");
            assert_eq!(
                body["error"], "non_production_refused",
                "{listing:?} {build}: {body}"
            );
            assert!(
                body["message"].as_str().unwrap_or("").contains("dev-a1"),
                "the refusal names the sandbox: {body}"
            );

            let token = publish_token(app_id);
            let (status, body) =
                listed_as(listing, staff(&t), token, app_id, of_build(build)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{listing:?} {build}: {body}");
            assert_eq!(
                body["error"], "publish_token_refused",
                "{listing:?} {build}: {body}"
            );

            let (status, body) = listed_as(listing, staff(&t), None, app_id, of_build(build)).await;
            assert_eq!(status, StatusCode::OK, "{listing:?} {build}: {body}");
            assert_eq!(ids(&body), vec![boxed.clone()], "{listing:?} {build}");
        }
    }

    // A build production serves is production's, whichever sandbox also
    // points at it.
    let production_uuid = build_pk(&t, app_id, PRODUCTION_BUILD).await;
    t.db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE app_environments SET build_id = $2 WHERE app_id = $1 AND name = 'dev-b2'",
        [app_id.into(), production_uuid.into()],
    ))
    .await
    .expect("point dev-b2 at production's build");
    for listing in [Listing::App, Listing::Function] {
        let token = publish_token(app_id);
        for (who, caller, token) in [
            ("staff", staff(&t), None),
            ("a tenant admin", outsider(), None),
            ("a publish token", staff(&t), token),
        ] {
            let query = of_build(PRODUCTION_BUILD);
            let (status, body) = listed_as(listing, caller, token, app_id, query).await;
            assert_eq!(status, StatusCode::OK, "{listing:?} {who}: {body}");
            assert_eq!(ids(&body), vec![live.clone()], "{listing:?} {who}");
        }
    }

    // Deleted: the pointer is cleared at once, so nothing serves the build.
    ops::delete(
        &t.db,
        &app,
        &sandbox("a1"),
        Some(&t.guest()),
        TeardownReason::Deleted,
    )
    .await
    .expect("delete the sandbox");
    for listing in [Listing::App, Listing::Function] {
        let query = of_build(SANDBOX_BUILD);
        let (status, body) = listed_as(listing, outsider(), None, app_id, query).await;
        assert_eq!(status, StatusCode::OK, "{listing:?}: {body}");
        assert_eq!(
            ids(&body),
            Vec::<String>::new(),
            "{listing:?}: no row leaks"
        );
    }
}
