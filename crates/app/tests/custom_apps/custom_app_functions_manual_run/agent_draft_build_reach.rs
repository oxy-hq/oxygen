//! `?build=` naming a draft a **sandbox agent token** published, on an app
//! unpublished since, is naming staging.
//!
//! An app with no production build runs staging's build on the production
//! path, so `invocation_reach`'s rule calls that build production's and a
//! caller without reach reads its rows. Not when a token published the
//! build: production does not fall back to it (`custom_apps_agent_built`), so
//! it is staging's alone — and the listing has to agree with the function
//! runtime about that. Both ask one thing,
//! `EnvironmentBuilds::production_runs`; the listing once asked a rule of
//! its own that knew nothing of the exception, and answered such a build as
//! production's (an empty list, where staging's build answers a refusal).
//!
//! The build is marked here as a token's publish marks it
//! (`app_builds.published_token_id`); the publish that writes the mark is
//! `sandbox_agent_token::staging_promote`'s.

use axum::http::StatusCode;
use oxy_app::server::api::admin::apps::handlers::unpublish_one;
use oxy_app::server::api::custom_apps_env_resolve::resolve_function_environment;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::json;
use uuid::Uuid;

use super::callers::{outsider, publish_token, staff};
use super::invocation_reach::{Listing, listed_as, of_build};
use super::readback::{APP, PRODUCTION_BUILD, STAGING_BUILD, build_pk, ids, ran, two_builds};
use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, publish_build, seeded_tenant};
use crate::sandbox_publish::app_row;
use crate::staging_functions::{make_guest_staff, staging_host};

const PERSONS_DRAFT: &str = "rb-person-2";
const WHOAMI_JS: &str =
    "export default async (req, ctx) => Response.json({ channel: ctx.channel });";

/// Marks `build` as one a sandbox agent token published.
async fn published_by_a_token(t: &Tenant, build: Uuid) {
    t.db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE app_builds SET published_token_id = $2 WHERE id = $1",
        [build.into(), Uuid::new_v4().into()],
    ))
    .await
    .expect("mark the build");
}

/// The build production's functions run for the app now.
async fn production_runs(t: &Tenant, app_id: Uuid) -> Option<Uuid> {
    let app = app_row(&t.db, app_id).await;
    resolve_function_environment(&t.db, &app, &AppEnvironment::Production)
        .await
        .expect("resolve production")
        .build_id
}

/// The app is unpublished after an agent's draft landed. Production runs
/// nothing, so naming the draft — by its publish id or its UUID — is naming
/// staging: refused to a caller without reach and to a publish token, and
/// listed to staff with reach. Once a person publishes a draft over it,
/// production falls back to theirs as it always did, naming theirs is naming
/// nothing non-production, and the agent's rows stay staging's.
#[tokio::test]
async fn naming_an_agents_draft_on_an_unpublished_app_is_naming_staging() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    make_guest_staff();
    // Marked before anything asks whose build it is: the answer is kept for
    // a minute once read.
    let draft = build_pk(&t, app_id, STAGING_BUILD).await;
    published_by_a_token(&t, draft).await;
    let staged = ran(&t, APP, "whoami", &staging_host(&t, APP))
        .await
        .to_string();
    unpublish_one(&t.db, app_id, t.guest_id)
        .await
        .map_err(|refused| refused.status)
        .expect("unpublish the app");
    assert_eq!(
        production_runs(&t, app_id).await,
        None,
        "the runtime runs nothing in production"
    );

    let draft_uuid = draft.to_string();
    for listing in [Listing::App, Listing::Function] {
        for build in [STAGING_BUILD, draft_uuid.as_str()] {
            let (status, body) =
                listed_as(listing, outsider(), None, app_id, of_build(build)).await;
            assert_eq!(status, StatusCode::FORBIDDEN, "{listing:?} {build}: {body}");
            assert_eq!(
                body["error"], "non_production_refused",
                "{listing:?} {build}: {body}"
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
            assert_eq!(ids(&body), vec![staged.clone()], "{listing:?} {build}");
        }
    }

    // A person publishes a draft over it. Production falls back to theirs, so
    // theirs is production's build; the agent's is a retained build nothing
    // serves, whose rows are still staging's and are not listed.
    let whoami = FunctionSpec {
        name: "whoami",
        manifest: json!({ "route": true }),
        js: WHOAMI_JS,
    };
    publish_build(
        &t,
        APP,
        demo_workspace_id(),
        PERSONS_DRAFT,
        false,
        &[whoami],
    )
    .await;
    let theirs = build_pk(&t, app_id, PERSONS_DRAFT).await;
    assert_eq!(production_runs(&t, app_id).await, Some(theirs));
    for listing in [Listing::App, Listing::Function] {
        let token = publish_token(app_id);
        for (who, caller, token) in [
            ("a tenant admin", outsider(), None),
            ("a publish token", staff(&t), token),
        ] {
            for build in [PERSONS_DRAFT, STAGING_BUILD] {
                let query = of_build(build);
                let (status, body) =
                    listed_as(listing, caller.clone(), token.clone(), app_id, query).await;
                assert_eq!(status, StatusCode::OK, "{listing:?} {who} {build}: {body}");
                assert_eq!(
                    ids(&body),
                    Vec::<String>::new(),
                    "{listing:?} {who} {build}: no staging row leaks"
                );
            }
        }
    }
}
