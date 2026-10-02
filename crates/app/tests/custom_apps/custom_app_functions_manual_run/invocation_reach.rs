//! Who may read which rows of the invocation listings.
//!
//! A row of a non-production environment is read only by a caller who may
//! open the app's non-production environments — never by a publish token, a
//! tenant's own admin, or staff without that reach — however the request
//! arrives at it:
//!
//! - **naming the environment** (`?environment=staging`) is refused;
//! - **naming a build** only a non-production environment serves is naming
//!   that environment, and is refused the same way; production's build is
//!   not;
//! - **naming nothing** returns production's rows alone, and a page is filled
//!   from them: the filter is in the query, not applied to a page already cut.
//!
//! Staff with reach read every environment's rows, as before. On both
//! listings. The handlers are called directly with the extractors the router
//! would build, for the reason `callers` gives.

use axum::Extension;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use oxy_app::server::api::admin::apps::invocations::InvocationQuery;
use oxy_app::server::api::admin::apps::{
    functions as admin_functions, invocations as admin_invocations,
};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AppPublishTokenAuth;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::Value;
use uuid::Uuid;

use super::answer;
use super::callers::{outsider, publish_token, staff};
use super::readback::{APP, PRODUCTION_BUILD, STAGING_BUILD, build_pk, ids, ran, two_builds};
use crate::custom_app_functions_fixture::{Tenant, seeded_tenant};
use crate::staging_functions::{make_guest_staff, production_host, staging_host};

/// Which of the two listings a caller asks.
#[derive(Clone, Copy, Debug)]
pub(super) enum Listing {
    /// `GET /admin/apps/{id}/invocations`
    App,
    /// `GET …/{id}/functions/whoami/invocations`
    Function,
}

fn in_environment(name: &str) -> InvocationQuery {
    InvocationQuery {
        environment: Some(name.to_string()),
        ..InvocationQuery::default()
    }
}

pub(super) fn of_build(build: &str) -> InvocationQuery {
    InvocationQuery {
        build: Some(build.to_string()),
        ..InvocationQuery::default()
    }
}

fn first(limit: u64) -> InvocationQuery {
    InvocationQuery {
        limit: Some(limit),
        ..InvocationQuery::default()
    }
}

/// A listing, called as the router would with these extractors: the status,
/// and the rows as a bare array whichever listing answered (or the refusal).
pub(super) async fn listed_as(
    listing: Listing,
    caller: AuthenticatedUserExtractor,
    token: Option<Extension<AppPublishTokenAuth>>,
    app_id: Uuid,
    query: InvocationQuery,
) -> (StatusCode, Value) {
    let query = Ok(Query(query));
    let rows = |rows| serde_json::to_value(rows).expect("the rows serialize");
    match listing {
        Listing::App => {
            match admin_invocations::list_app_invocations(caller, token, Path(app_id), query).await
            {
                Ok(axum::Json(list)) => (StatusCode::OK, rows(list.invocations)),
                Err(refused) => answer(refused.into_response()).await,
            }
        }
        Listing::Function => {
            let path = Path((app_id, "whoami".to_string()));
            match admin_functions::list_invocations(caller, token, path, query).await {
                Ok(axum::Json(list)) => (StatusCode::OK, rows(list)),
                Err(refused) => answer(refused.into_response()).await,
            }
        }
    }
}

/// Which environment each listed row ran in, in the order listed.
fn environments(listed: &Value) -> Vec<String> {
    listed
        .as_array()
        .expect("a list of invocations")
        .iter()
        .map(|row| {
            row["environment"]
                .as_str()
                .expect("environment")
                .to_string()
        })
        .collect()
}

/// Rewrites one recorded invocation as a sandbox's. Functions do not run in a
/// dev slot yet, so no call writes such a row.
async fn move_to_sandbox(t: &Tenant, invocation: Uuid) {
    t.db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE app_function_invocations SET environment = 'dev-a1' WHERE id = $1",
        [invocation.into()],
    ))
    .await
    .expect("move the row");
}

/// A non-production environment's listing is that environment's read-back,
/// so naming one is the second door every such route has: a publish token is
/// refused whoever minted it, and a caller who may not open the app's
/// non-production environments is refused. On both listings.
#[tokio::test]
async fn listing_a_non_production_environment_needs_reach_and_refuses_a_publish_token() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    make_guest_staff();
    let staged = ran(&t, APP, "whoami", &staging_host(&t, APP))
        .await
        .to_string();

    for listing in [Listing::App, Listing::Function] {
        for environment in ["staging", "dev-a1"] {
            let (status, body) = listed_as(
                listing,
                outsider(),
                None,
                app_id,
                in_environment(environment),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{listing:?} {environment}: {body}"
            );
            assert_eq!(
                body["error"], "non_production_refused",
                "{listing:?} {environment}: {body}"
            );

            // Refused before the reach lookup: the token rides staff's own login.
            let token = publish_token(app_id);
            let (status, body) = listed_as(
                listing,
                staff(&t),
                token,
                app_id,
                in_environment(environment),
            )
            .await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "{listing:?} {environment}: {body}"
            );
            assert_eq!(
                body["error"], "publish_token_refused",
                "{listing:?} {environment}: {body}"
            );
        }

        // Staff with reach, on their own login, read it.
        let (status, body) =
            listed_as(listing, staff(&t), None, app_id, in_environment("staging")).await;
        assert_eq!(status, StatusCode::OK, "{listing:?}: {body}");
        assert_eq!(ids(&body), vec![staged.clone()], "{listing:?}");

        // A name that is not an environment is a 400 for anyone, before either door.
        let token = publish_token(app_id);
        let (status, body) = listed_as(
            listing,
            outsider(),
            token,
            app_id,
            in_environment("Staging"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{listing:?}: {body}");
        assert_eq!(body["error"], "invalid_environment", "{listing:?}");
    }
}

/// Three production calls, then three outside production (the last moved to
/// a sandbox): unfiltered and newest first, the non-production rows lead.
/// Returns the ids, oldest first.
async fn three_production_then_three_outside(t: &Tenant) -> Vec<String> {
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(ran(t, APP, "whoami", &production_host(t, APP)).await);
    }
    for _ in 0..3 {
        ids.push(ran(t, APP, "whoami", &staging_host(t, APP)).await);
    }
    move_to_sandbox(t, ids[5]).await;
    ids.iter().map(Uuid::to_string).collect()
}

/// Naming no environment is not a way around the door. A caller without
/// reach and a publish token are listed production's rows only, and the page
/// is full: the newest rows are all non-production here, so a filter applied
/// after `LIMIT` would hand back a short or empty page. Staff with reach read
/// every environment's rows, as before.
#[tokio::test]
async fn the_unfiltered_listing_returns_non_production_rows_only_to_a_caller_with_reach() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    make_guest_staff();
    let oldest_first = three_production_then_three_outside(&t).await;
    let newest =
        |n: usize, of: &[String]| -> Vec<String> { of.iter().rev().take(n).cloned().collect() };
    let (production, outside) = oldest_first.split_at(3);

    for listing in [Listing::App, Listing::Function] {
        // Staff with reach: every environment, newest first — the page of two
        // is the two newest rows, both non-production.
        let (status, all) = listed_as(listing, staff(&t), None, app_id, first(50)).await;
        assert_eq!(status, StatusCode::OK, "{listing:?}: {all}");
        assert_eq!(ids(&all), newest(6, &oldest_first), "{listing:?}");
        assert_eq!(
            environments(&all),
            [
                "dev-a1",
                "staging",
                "staging",
                "production",
                "production",
                "production"
            ],
            "{listing:?}"
        );
        let (_, page) = listed_as(listing, staff(&t), None, app_id, first(2)).await;
        assert_eq!(ids(&page), newest(2, outside), "{listing:?}");

        let token = publish_token(app_id);
        for (who, caller, token) in [
            ("a tenant admin", outsider(), None),
            ("a publish token", staff(&t), token),
        ] {
            let (status, all) =
                listed_as(listing, caller.clone(), token.clone(), app_id, first(50)).await;
            assert_eq!(status, StatusCode::OK, "{listing:?} {who}: {all}");
            assert_eq!(ids(&all), newest(3, production), "{listing:?} {who}");
            assert_eq!(
                environments(&all),
                ["production", "production", "production"],
                "{listing:?} {who}: no staging or sandbox row"
            );

            // The page is filled from production's rows, not cut short.
            let (status, page) =
                listed_as(listing, caller.clone(), token.clone(), app_id, first(2)).await;
            assert_eq!(status, StatusCode::OK, "{listing:?} {who}: {page}");
            assert_eq!(ids(&page), newest(2, production), "{listing:?} {who}");

            // Naming production is the same read, for them and for staff.
            let (_, named) =
                listed_as(listing, caller, token, app_id, in_environment("production")).await;
            assert_eq!(named, all, "{listing:?} {who}");
        }
        let (_, named) = listed_as(
            listing,
            staff(&t),
            None,
            app_id,
            in_environment("production"),
        )
        .await;
        assert_eq!(ids(&named), newest(3, production), "{listing:?}");
    }
}

/// `?build=` is a second way to ask for an environment's rows. Naming the
/// build staging serves — by its publish id or its UUID — is naming staging:
/// refused to a caller without reach and to a publish token. Naming the build
/// production serves is naming nothing non-production, and a build the app
/// does not have is the empty list it always was.
#[tokio::test]
async fn naming_a_build_only_staging_serves_needs_reach_and_productions_build_does_not() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    make_guest_staff();
    let live = ran(&t, APP, "whoami", &production_host(&t, APP))
        .await
        .to_string();
    let staged = ran(&t, APP, "whoami", &staging_host(&t, APP))
        .await
        .to_string();
    let staging_uuid = build_pk(&t, app_id, STAGING_BUILD).await.to_string();

    for listing in [Listing::App, Listing::Function] {
        for build in [STAGING_BUILD, staging_uuid.as_str()] {
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

        let token = publish_token(app_id);
        for (who, caller, token) in [
            ("staff", staff(&t), None),
            ("a tenant admin", outsider(), None),
            ("a publish token", staff(&t), token),
        ] {
            let query = of_build(PRODUCTION_BUILD);
            let (status, body) =
                listed_as(listing, caller.clone(), token.clone(), app_id, query).await;
            assert_eq!(status, StatusCode::OK, "{listing:?} {who}: {body}");
            assert_eq!(ids(&body), vec![live.clone()], "{listing:?} {who}");

            let query = of_build("no-such-build");
            let (status, body) = listed_as(listing, caller, token, app_id, query).await;
            assert_eq!(status, StatusCode::OK, "{listing:?} {who}: {body}");
            assert_eq!(ids(&body), Vec::<String>::new(), "{listing:?} {who}");
        }
    }
}
