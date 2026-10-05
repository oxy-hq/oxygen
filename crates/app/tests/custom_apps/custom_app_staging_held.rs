//! The console's held list (`GET /api/customer-apps/{id}/staging/held`,
//! `custom_apps_staging_held`): what a developer's own staging requests held,
//! read back from the one `app.staging.held` writer.
//!
//! Every held row here comes from a real staging `/fn` invocation through the
//! serve route; the list handler is called directly with the extractors the
//! router would build (the mount's PlatformApps + scope guard is `authz`'s).
//!
//! - the invoker sees its row, with the app and the actor stamped;
//! - another developer who may open staging sees `[]` (the rule is per person);
//! - a caller oxy-authz does not let open staging gets 404;
//! - a production invocation adds no row;
//! - a row written before `app_id` was stamped is found by slug within the org.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::StatusCode;
use entity::users::UserStatus;
use oxy_app::server::api::custom_apps_staging_held::{HeldEntry, HeldQuery, list_held};
use oxy_app_core::audit::{ActorType, AuditEntry};
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::types::AuthenticatedUser;
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use serde_json::json;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{Tenant, publish_build, seeded_tenant};
use crate::staging_functions::{call_on, held_rows, one, production_host, staging_host};

/// A second Oxy developer, staff by the same `OXY_OWNER` list as the guest.
const OTHER_DEV: &str = "other-dev@oxy.tech";

/// Both developers are staff. Before any staging call: the non-production
/// decision is cached per (user, app) for a minute.
fn two_staff() {
    // SAFETY: nextest runs each test in its own process.
    unsafe { std::env::set_var("OXY_OWNER", format!("{LOCAL_GUEST_EMAIL},{OTHER_DEV}")) };
}

fn user(id: Uuid, email: &str) -> AuthenticatedUserExtractor {
    AuthenticatedUserExtractor(AuthenticatedUser {
        id,
        email: Some(email.to_string()),
        name: "Developer".to_string(),
        picture: None,
        status: UserStatus::Active,
    })
}

async fn list_as(
    app: Uuid,
    who: AuthenticatedUserExtractor,
) -> Result<Vec<HeldEntry>, (StatusCode, String)> {
    list_held(Path(app), Query(HeldQuery::default()), who)
        .await
        .map(|Json(rows)| rows)
}

/// A route function that tries one write `ctx.fetch` (a POST to a name that
/// cannot resolve, so production needs no network) and answers either way.
const POST_JS: &str = r#"
export default async (req, ctx) => {
  try { await ctx.fetch("https://staging-probe.invalid/orders", { method: "POST", body: "{}" }); }
  catch (e) {}
  return Response.json({ ok: true });
};
"#;

/// Publish `app` with the POST function, promoted, so it runs on both hosts.
async fn post_app(t: &Tenant, app: &str) -> Uuid {
    let functions = one(
        "save",
        json!({ "route": true, "timeoutSeconds": 30 }),
        POST_JS,
    );
    publish_build(t, app, demo_workspace_id(), "held-1", true, &functions)
        .await
        .app_id
}

#[tokio::test]
async fn the_staging_invoker_lists_its_own_held_row() {
    let t = seeded_tenant().await;
    let app = "stg-held-own";
    let app_id = post_app(&t, app).await;
    two_staff();

    call_on(&t, app, "save", &staging_host(&t, app), &[]).await;

    let rows = list_as(app_id, user(t.guest_id, LOCAL_GUEST_EMAIL))
        .await
        .expect("the invoker may open staging");
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].function, "save");
    let w = &rows[0].writes[0];
    assert_eq!(
        (
            w.plane.as_str(),
            w.namespace.as_str(),
            w.verb.as_str(),
            w.op.as_deref()
        ),
        ("fetch", "staging-probe.invalid", "POST", Some("fetch"))
    );

    let audit = held_rows(&t).await;
    assert_eq!(audit.len(), 1);
    assert_eq!(audit[0].actor_user_id, Some(t.guest_id), "the column");
    assert_eq!(audit[0].environment, "staging");
    assert_eq!(audit[0].metadata["app_id"], json!(app_id));
    assert_eq!(audit[0].metadata["actor_user_id"], json!(t.guest_id));
}

#[tokio::test]
async fn another_staging_developer_sees_none_of_the_invokers_rows() {
    let t = seeded_tenant().await;
    let app = "stg-held-other";
    let app_id = post_app(&t, app).await;
    two_staff();

    call_on(&t, app, "save", &staging_host(&t, app), &[]).await;
    assert_eq!(held_rows(&t).await.len(), 1, "the invoker's row exists");

    let rows = list_as(app_id, user(Uuid::new_v4(), OTHER_DEV))
        .await
        .expect("the other developer may open staging");
    assert!(rows.is_empty(), "held rows are per person: {rows:?}");
}

#[tokio::test]
async fn a_caller_who_may_not_open_staging_gets_404() {
    let t = seeded_tenant().await;
    let app = "stg-held-404";
    let app_id = post_app(&t, app).await;
    two_staff();
    call_on(&t, app, "save", &staging_host(&t, app), &[]).await;

    let (code, _) = list_as(app_id, user(Uuid::new_v4(), "admin@customer.example"))
        .await
        .expect_err("not staff");
    assert_eq!(code, StatusCode::NOT_FOUND);
    let (code, _) = list_as(Uuid::new_v4(), user(t.guest_id, LOCAL_GUEST_EMAIL))
        .await
        .expect_err("no such app");
    assert_eq!(
        code,
        StatusCode::NOT_FOUND,
        "the same answer as a missing app"
    );
}

#[tokio::test]
async fn a_production_invocation_adds_no_held_row() {
    let t = seeded_tenant().await;
    let app = "stg-held-prod";
    let app_id = post_app(&t, app).await;
    two_staff();

    call_on(&t, app, "save", &production_host(&t, app), &[]).await;

    assert!(held_rows(&t).await.is_empty(), "production holds nothing");
    let rows = list_as(app_id, user(t.guest_id, LOCAL_GUEST_EMAIL))
        .await
        .expect("staff");
    assert!(rows.is_empty(), "{rows:?}");
}

/// A row as the host wrote it before `app_id` was stamped.
async fn record_legacy_row(t: &Tenant, app_slug: &str, function: &str) {
    let entry = AuditEntry::new(LOCAL_GUEST_EMAIL, "app.staging.held")
        .actor(t.guest_id, ActorType::User)
        .org(t.org_id)
        .workspace(demo_workspace_id())
        .metadata(json!({
            "app_slug": app_slug,
            "function": function,
            "invocation_id": Uuid::new_v4(),
            "mode": "route",
            "request_id": null,
            "trace_id": null,
            "writes": [{ "plane": "oltp", "namespace": "app_x", "verb": "INSERT",
                         "table": "orders", "rows": null, "statements": 1, "op": "oltp.execute" }],
        }))
        .environment("staging");
    oxy_app_core::audit::record(&t.db, entry)
        .await
        .expect("legacy row");
}

#[tokio::test]
async fn a_row_written_before_app_id_was_stamped_is_found_by_slug_and_org() {
    let t = seeded_tenant().await;
    let app = "stg-held-legacy";
    let app_id = post_app(&t, app).await;
    two_staff();

    record_legacy_row(&t, app, "before").await;
    // Same slug, another org's row: never this app's.
    let other_org = AuditEntry::new(LOCAL_GUEST_EMAIL, "app.staging.held")
        .actor(t.guest_id, ActorType::User)
        .org(Uuid::new_v4())
        .metadata(json!({ "app_slug": app, "function": "elsewhere", "writes": [] }))
        .environment("staging");
    oxy_app_core::audit::record(&t.db, other_org)
        .await
        .expect("other org's row");
    call_on(&t, app, "save", &staging_host(&t, app), &[]).await;

    let rows = list_as(app_id, user(t.guest_id, LOCAL_GUEST_EMAIL))
        .await
        .expect("staff");
    let functions: Vec<&str> = rows.iter().map(|r| r.function.as_str()).collect();
    assert_eq!(
        functions,
        vec!["save", "before"],
        "newest first, by slug and org"
    );
    assert_eq!(rows[1].writes[0].table, "orders");
}
