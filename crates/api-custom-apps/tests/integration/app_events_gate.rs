//! The gate on shipping an event publisher's build: a build that removes an
//! event type, or changes its schema incompatibly, is refused while a function
//! in production listens to it. A listener outside production only warns, a
//! rollback is never refused, and `break_listeners` ships over a refusal and
//! says so in the audit log.
//!
//! The publisher is the app `locations` (event `statement.uploaded`); the
//! listener is `bookkeeping`'s function `on-statement-uploaded`.

use entity::{app_builds, apps, audit_events};
use oxy_api_custom_apps::app_events::gate::{BROKE_LISTENERS, GateReport, GateScope, check};
use oxy_api_custom_apps::app_events::subscriptions::listeners;
use oxy_api_custom_apps::custom_apps_promote::PromoteError;
use oxy_api_custom_apps::custom_apps_promote::act;
use oxy_api_custom_apps::custom_apps_promote::resolve::{PromoteRequest, RollbackRequest};
use oxy_api_custom_apps::custom_apps_publish::{
    OrgRef, PublishError, PublishInput, PublishOptions, PublishResult, PublishTarget, publish_with,
};
use oxy_auth::user::LOCAL_GUEST_EMAIL;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, QueryOrder};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::app_events_publish::enable_app_events;
use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, Tenant, seeded_tenant};
use crate::sandbox_publish::bundle;
use crate::staging_functions::make_guest_staff;

const PUBLISHER: &str = "locations";
const LISTENER: &str = "bookkeeping";
const EVENT: &str = "statement.uploaded";
const JS: &str = "export default async () => Response.json({});";

const REMOVED: &str = "`locations/statement.uploaded` is removed, but \
    bookkeeping/on-statement-uploaded listens to it in production. Ship a build that keeps it, \
    or pass --break-listeners.";
const INCOMPATIBLE: &str = "`locations/statement.uploaded` changes incompatibly (`period` was \
    removed), and bookkeeping/on-statement-uploaded listens to it in production.";

fn schema(properties: Value) -> Value {
    json!({ "type": "object", "properties": properties })
}

/// A manifest declaring the one event with `properties`.
fn declaring(properties: Value) -> Value {
    json!({ "events": { EVENT: { "schema": schema(properties) } } })
}

fn period() -> Value {
    json!({ "period": { "type": "string" } })
}

async fn ship(
    t: &Tenant,
    slug: &str,
    build: &str,
    promote: bool,
    manifest: Option<Value>,
    functions: &[FunctionSpec],
    options: PublishOptions,
) -> Result<PublishResult, PublishError> {
    let input = PublishInput {
        org_ref: Some(OrgRef::Id(t.org_id)),
        app_slug: slug.to_string(),
        project_id: demo_workspace_id(),
        branch: None,
        build_id: build.to_string(),
        name: None,
        promote,
        tarball: bundle(slug, functions, manifest.unwrap_or_else(|| json!({})), &[]),
        manifest: None,
        source_repo: None,
        commit_sha: None,
        published_by: Some(t.guest_id),
        published_by_email: Some(LOCAL_GUEST_EMAIL.to_string()),
        publisher: Some(oxy_server_authz::Caller::without_credential(
            t.guest_id,
            LOCAL_GUEST_EMAIL,
        )),
        machine_app_id: None,
        published_via: None,
        semantic_revision_id: None,
        confirm_schedule_removals: Vec::new(),
    };
    publish_with(input, PublishTarget::Channels, options).await
}

async fn publish_events(
    t: &Tenant,
    build: &str,
    promote: bool,
    manifest: Option<Value>,
) -> PublishResult {
    ship(
        t,
        PUBLISHER,
        build,
        promote,
        manifest,
        &[],
        PublishOptions::default(),
    )
    .await
    .expect("the publisher's build ships")
}

async fn publish_listener(t: &Tenant, promote: bool) {
    publish_listener_of(t, PUBLISHER, promote).await;
}

async fn publish_listener_of(t: &Tenant, publisher: &str, promote: bool) {
    let listener = FunctionSpec {
        name: "on-statement-uploaded",
        manifest: json!({ "on": format!("{publisher}/{EVENT}") }),
        js: JS,
    };
    ship(
        t,
        LISTENER,
        "l1",
        promote,
        None,
        &[listener],
        PublishOptions::default(),
    )
    .await
    .expect("the listener's build ships");
}

async fn app_of(db: &DatabaseConnection, id: Uuid) -> apps::Model {
    apps::Entity::find_by_id(id)
        .one(db)
        .await
        .expect("read the app")
        .expect("the app")
}

async fn build_of(db: &DatabaseConnection, app: Uuid, build: &str) -> app_builds::Model {
    app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app))
        .filter(app_builds::Column::BuildId.eq(build))
        .one(db)
        .await
        .expect("read the build")
        .expect("the build")
}

async fn production_build(db: &DatabaseConnection, app: Uuid) -> Option<Uuid> {
    crate::app_environments::serving(db, app).await.production
}

async fn audit(t: &Tenant, action: &str) -> Vec<audit_events::Model> {
    audit_events::Entity::find()
        .filter(audit_events::Column::Action.eq(action))
        .filter(audit_events::Column::OrgId.eq(t.org_id))
        .order_by_asc(audit_events::Column::Seq)
        .all(&t.db)
        .await
        .expect("read the audit rows")
}

/// `locations` serves `period` in production and `bookkeeping` listens there,
/// with app events on for the org.
async fn listened_to() -> (Tenant, Uuid) {
    let t = seeded_tenant().await;
    enable_app_events(&t, &[t.org_id]).await;
    let publisher = publish_events(&t, "p1", true, Some(declaring(period()))).await;
    publish_listener(&t, true).await;
    (t, publisher.app_id)
}

fn promote_req(build: &str, break_listeners: bool) -> PromoteRequest {
    PromoteRequest {
        from: "staging".into(),
        build: Some(build.into()),
        break_listeners,
        ..Default::default()
    }
}

#[tokio::test]
async fn removing_a_type_with_a_production_listener_is_refused() {
    let (t, app_id) = listened_to().await;
    let app = app_of(&t.db, app_id).await;

    // The decision, for a build that drops the event, and for one that drops `period`.
    publish_events(&t, "p2", false, Some(json!({ "events": {} }))).await;
    let removed = build_of(&t.db, app_id, "p2").await;
    let report = check(
        &t.db,
        &app,
        &removed,
        GateScope::Ship {
            break_listeners: false,
        },
    )
    .await
    .expect("check");
    assert_eq!(report.refusals, vec![REMOVED.to_string()]);
    // The same listener's staging subscription (promoted builds serve both) only warns.
    assert_eq!(
        report.warnings,
        vec![REMOVED.replace("in production", "in staging")]
    );

    publish_events(&t, "p3", false, Some(declaring(json!({})))).await;
    let narrowed = build_of(&t.db, app_id, "p3").await;
    let report = check(
        &t.db,
        &app,
        &narrowed,
        GateScope::Ship {
            break_listeners: false,
        },
    )
    .await
    .expect("check");
    assert_eq!(report.refusals, vec![INCOMPATIBLE.to_string()]);

    // A promoting publish is refused, and production stays where it was.
    let live = production_build(&t.db, app_id).await;
    let refused = ship(
        &t,
        PUBLISHER,
        "p4",
        true,
        Some(json!({ "events": {} })),
        &[],
        PublishOptions::default(),
    )
    .await
    .expect_err("the publish breaks a production listener");
    let PublishError::EventListenersBroken(messages) = &refused else {
        panic!("{refused:?}");
    };
    assert_eq!(messages, &vec![REMOVED.to_string()]);
    assert_eq!(production_build(&t.db, app_id).await, live);

    // So is the promote: the plan lists it, the act refuses with its code.
    make_guest_staff();
    let dry = act::promote(
        &t.db,
        &t.guest(),
        app_id,
        "production",
        &PromoteRequest {
            dry_run: true,
            ..promote_req("p2", false)
        },
    )
    .await
    .expect("a dry run answers");
    let codes: Vec<&str> = dry.blockers.iter().map(|b| b.code).collect();
    assert_eq!(codes, vec!["event_listeners_broken"]);
    assert_eq!(dry.blockers[0].message, REMOVED);
    let refused = act::promote(
        &t.db,
        &t.guest(),
        app_id,
        "production",
        &promote_req("p2", false),
    )
    .await
    .expect_err("the promote breaks a production listener");
    let PromoteError::EventListenersBroken(messages) = &refused else {
        panic!("{refused:?}");
    };
    assert_eq!(messages, &vec![REMOVED.to_string()]);
    assert_eq!(refused.status().as_u16(), 409);
    assert_eq!(refused.code(), "event_listeners_broken");
    assert_eq!(production_build(&t.db, app_id).await, live);
    assert!(audit(&t, BROKE_LISTENERS).await.is_empty());
}

/// R31: with `app_events` off for the org nothing is ever delivered to a
/// listener, so the gate refuses and warns nothing: a promote over a
/// would-be-broken production listener goes ahead, unaudited. The
/// subscriptions still reconcile (platform metadata only), so turning the
/// flag on gates the next ship at once.
#[tokio::test]
async fn a_flag_off_orgs_promote_over_a_would_be_broken_listener_succeeds() {
    let t = seeded_tenant().await;
    let publisher = publish_events(&t, "p1", true, Some(declaring(period()))).await;
    publish_listener(&t, true).await;
    let app_id = publisher.app_id;
    assert!(
        !listeners(
            &t.db,
            t.org_id,
            "production",
            &format!("{PUBLISHER}/{EVENT}")
        )
        .await
        .expect("read the listeners")
        .is_empty(),
        "the subscription reconciled with the flag off"
    );

    publish_events(&t, "p2", false, Some(json!({ "events": {} }))).await;
    let removed = build_of(&t.db, app_id, "p2").await;
    let report = check(
        &t.db,
        &app_of(&t.db, app_id).await,
        &removed,
        GateScope::Ship {
            break_listeners: false,
        },
    )
    .await
    .expect("check");
    assert_eq!(report, GateReport::default(), "neither refused nor warned");

    make_guest_staff();
    act::promote(
        &t.db,
        &t.guest(),
        app_id,
        "production",
        &promote_req("p2", false),
    )
    .await
    .expect("the promote goes ahead");
    assert_eq!(production_build(&t.db, app_id).await, Some(removed.id));
    assert!(audit(&t, BROKE_LISTENERS).await.is_empty());
}

#[tokio::test]
async fn break_listeners_overrides_and_is_audited() {
    let (t, app_id) = listened_to().await;
    make_guest_staff();

    // The promote.
    publish_events(&t, "p2", false, Some(json!({ "events": {} }))).await;
    let moved = act::promote(
        &t.db,
        &t.guest(),
        app_id,
        "production",
        &promote_req("p2", true),
    )
    .await
    .expect("--break-listeners ships it");
    assert!(moved.moved);
    assert!(
        moved.warnings.iter().any(|w| w == REMOVED),
        "{:?}",
        moved.warnings
    );
    let p2 = build_of(&t.db, app_id, "p2").await;
    assert_eq!(production_build(&t.db, app_id).await, Some(p2.id));
    let rows = audit(&t, BROKE_LISTENERS).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].metadata["build"], "p2");
    assert_eq!(rows[0].metadata["broken"], json!([REMOVED]));
    assert_eq!(rows[0].environment, "production");
}

#[tokio::test]
async fn a_publish_with_break_listeners_reports_what_it_broke() {
    let (t, app_id) = listened_to().await;
    let options = PublishOptions {
        break_listeners: true,
    };
    let shipped = ship(
        &t,
        PUBLISHER,
        "p2",
        true,
        Some(json!({ "events": {} })),
        &[],
        options,
    )
    .await
    .expect("a publish with --break-listeners goes ahead");
    assert_eq!(shipped.broken_listeners, vec![REMOVED.to_string()]);
    assert!(
        shipped.warnings.iter().any(|w| w == REMOVED),
        "{:?}",
        shipped.warnings
    );
    let p2 = build_of(&t.db, app_id, "p2").await;
    assert_eq!(production_build(&t.db, app_id).await, Some(p2.id));
}

#[tokio::test]
async fn a_staging_listener_only_warns() {
    let t = seeded_tenant().await;
    enable_app_events(&t, &[t.org_id]).await;
    let publisher = publish_events(&t, "p1", true, Some(declaring(period()))).await;
    // Not promoted: the listener is subscribed in staging only.
    publish_listener(&t, false).await;
    let app = app_of(&t.db, publisher.app_id).await;

    publish_events(&t, "p2", false, Some(json!({ "events": {} }))).await;
    let removed = build_of(&t.db, publisher.app_id, "p2").await;
    let report = check(
        &t.db,
        &app,
        &removed,
        GateScope::Ship {
            break_listeners: false,
        },
    )
    .await
    .expect("check");
    assert!(report.refusals.is_empty(), "{:?}", report.refusals);
    assert_eq!(
        report.warnings,
        vec![REMOVED.replace("in production", "in staging")]
    );

    // And a promoting publish goes through, carrying the warning.
    let shipped = ship(
        &t,
        PUBLISHER,
        "p3",
        true,
        Some(json!({ "events": {} })),
        &[],
        PublishOptions::default(),
    )
    .await
    .expect("a staging listener does not stop a ship");
    assert!(
        shipped
            .warnings
            .iter()
            .any(|w| w.contains("listens to it in staging")),
        "{:?}",
        shipped.warnings
    );
    assert!(shipped.broken_listeners.is_empty());
}

#[tokio::test]
async fn rollback_is_never_refused() {
    let (t, app_id) = listened_to().await;
    let app = app_of(&t.db, app_id).await;
    make_guest_staff();

    // Production moves to a build that adds an optional field (compatible).
    let wider = json!({ "period": { "type": "string" }, "note": { "type": "string" } });
    publish_events(&t, "p2", true, Some(declaring(wider))).await;
    let p1 = build_of(&t.db, app_id, "p1").await;

    // Rolling back to the narrower schema removes `note`: a refusal as a ship,
    // a warning as a rollback.
    let ship_report = check(
        &t.db,
        &app,
        &p1,
        GateScope::Ship {
            break_listeners: false,
        },
    )
    .await
    .expect("check");
    assert_eq!(ship_report.refusals.len(), 1, "{ship_report:?}");
    let report = check(&t.db, &app, &p1, GateScope::Rollback)
        .await
        .expect("check");
    assert!(report.refusals.is_empty(), "{:?}", report.refusals);
    assert!(
        ship_report
            .refusals
            .iter()
            .all(|r| report.warnings.contains(r)),
        "{report:?}"
    );

    let plan = act::rollback(
        &t.db,
        &t.guest(),
        app_id,
        "production",
        &RollbackRequest {
            to: Some("p1".into()),
            ..Default::default()
        },
    )
    .await
    .expect("a rollback is not refused");
    assert!(plan.moved);
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.contains("`note` was removed")),
        "{:?}",
        plan.warnings
    );
    assert_eq!(production_build(&t.db, app_id).await, Some(p1.id));
    // The rollback's audit row records what it broke.
    let rows = audit(&t, "app.rolled_back").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let broken = rows[0].metadata["broken_listeners"]
        .as_array()
        .expect("recorded");
    assert!(
        broken.iter().any(|b| b
            .as_str()
            .is_some_and(|b| b.contains("`note` was removed") && b.contains("in production"))),
        "{broken:?}"
    );
}

#[tokio::test]
async fn an_additive_schema_change_passes() {
    let (t, app_id) = listened_to().await;
    let app = app_of(&t.db, app_id).await;
    let wider = json!({ "period": { "type": "string" }, "note": { "type": "string" } });
    publish_events(&t, "p2", false, Some(declaring(wider))).await;
    let target = build_of(&t.db, app_id, "p2").await;
    let report = check(
        &t.db,
        &app,
        &target,
        GateScope::Ship {
            break_listeners: false,
        },
    )
    .await
    .expect("check");
    assert!(report.refusals.is_empty(), "{:?}", report.refusals);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);

    publish_events(
        &t,
        "p3",
        true,
        Some(declaring(json!({
            "period": { "type": "string" }, "note": { "type": "string" }
        }))),
    )
    .await;
    assert_eq!(
        production_build(&t.db, app_id).await,
        Some(build_of(&t.db, app_id, "p3").await.id)
    );
}

/// The console's Make Live has no override: it is refused with the gate's
/// code, tells the user where the override is, and production stays put.
#[tokio::test]
async fn console_make_live_is_refused_over_a_production_listener() {
    use axum::response::IntoResponse;
    use oxy_api_custom_apps::admin::apps::handlers as admin_apps;
    use oxy_api_custom_apps::custom_apps_promote::AuditedBy;

    let (t, app_id) = listened_to().await;
    let live = production_build(&t.db, app_id).await;
    // The draft, as a publish leaves it: staging serves it.
    publish_events(&t, "p2", false, Some(json!({ "events": {} }))).await;

    let refused = admin_apps::publish_one(&t.db, app_id, &t.guest(), AuditedBy::Caller)
        .await
        .expect_err("Make Live of a build that removes a listened-to event");
    let response = admin_apps::PromoteRefusal::from(refused).into_response();
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("a body");
    let body: Value = serde_json::from_slice(&bytes).expect("JSON");
    assert_eq!(body["code"], "event_listeners_broken", "{body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert!(message.contains(REMOVED), "{message}");
    assert!(
        message.contains("oxyc publish --break-listeners"),
        "{message}"
    );
    assert_eq!(production_build(&t.db, app_id).await, live);
}

/// The console's rollback is never refused: it moves production, and the
/// listener it breaks is a warning (logged; the check's report says which).
#[tokio::test]
async fn console_rollback_succeeds_over_a_production_listener() {
    let agent = crate::sandbox_agent_token::fixture::agent().await;
    let t = &agent.t;
    enable_app_events(t, &[t.org_id]).await;
    let slug = agent.app.slug.clone();
    let wider = json!({ "period": { "type": "string" }, "note": { "type": "string" } });
    let narrow = ship(
        t,
        &slug,
        "e1",
        true,
        Some(declaring(period())),
        &[],
        PublishOptions::default(),
    )
    .await
    .expect("e1");
    publish_listener_of(t, &slug, true).await;
    ship(
        t,
        &slug,
        "e2",
        true,
        Some(declaring(wider)),
        &[],
        PublishOptions::default(),
    )
    .await
    .expect("e2 widens the schema and ships");
    let e1 = build_of(&t.db, narrow.app_id, "e1").await;
    let report = check(&t.db, &agent.app, &e1, GateScope::Rollback)
        .await
        .expect("check");
    assert!(report.refusals.is_empty());
    assert!(
        report
            .warnings
            .iter()
            .any(|w| w.contains("`note` was removed") && w.contains("in production")),
        "{:?}",
        report.warnings
    );

    let uri = format!("/customer-apps/{}/rollback", agent.app.id);
    let (status, body) =
        crate::promote_rollback::person(&agent, "POST", &uri, Some(json!({ "build_id": e1.id })))
            .await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    assert_eq!(production_build(&t.db, agent.app.id).await, Some(e1.id));
    // The person who clicked is told, and the audit row says what it broke.
    let warnings = body["warnings"].as_array().expect("the response warns");
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().is_some_and(|w| w.contains("`note` was removed"))),
        "{body}"
    );
    let rows = audit(t, "app.rolled_back").await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(
        rows[0].metadata["broken_listeners"]
            .as_array()
            .is_some_and(|b| b
                .iter()
                .any(|b| b.as_str().is_some_and(|b| b.contains("`note` was removed")))),
        "{:?}",
        rows[0].metadata
    );
}

/// `oxyc publish --promote --break-listeners` (`breakListeners` in the form)
/// through the real route: refused without it, shipped with it, and the row
/// that says which listeners it broke is in the audit table.
#[tokio::test]
async fn a_publish_with_break_listeners_writes_the_override_audit_row() {
    use crate::sandbox_publish_route::publish;

    let (t, app_id) = listened_to().await;
    // The route's bundle declares no events: it removes the one in production.
    let (status, body) =
        publish(PUBLISHER, &[("build_id", "r1"), ("promote", "true")], false).await;
    assert_eq!(status, axum::http::StatusCode::CONFLICT, "{body}");
    assert!(body.contains(REMOVED), "{body}");
    assert!(audit(&t, BROKE_LISTENERS).await.is_empty());

    let fields = [
        ("build_id", "r2"),
        ("promote", "true"),
        ("breakListeners", "true"),
    ];
    let (status, body) = publish(PUBLISHER, &fields, false).await;
    assert_eq!(status, axum::http::StatusCode::OK, "{body}");
    let r2 = build_of(&t.db, app_id, "r2").await;
    assert_eq!(production_build(&t.db, app_id).await, Some(r2.id));

    let rows = audit(&t, BROKE_LISTENERS).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row.actor_user_id, Some(t.guest_id));
    assert_eq!(row.environment, "production");
    assert_eq!(row.target_label, Some(format!("{PUBLISHER}/production")));
    assert_eq!(row.metadata["build"], "r2");
    assert_eq!(row.metadata["broken"], json!([REMOVED]));
}
