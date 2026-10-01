//! A failing **staging** function neither pages nor claims production's
//! first-occurrence slot (environments design §3.4; Tay's condition, and the
//! gate on Phase 2 of that design) — shown end to end, through real staging
//! calls, with the ops Slack target configured so the pager would act on any
//! failure it was handed.
//!
//! `function_failure_alerts` pins the SQL half (`claim` counts production's
//! invocations only) over hand-inserted rows. This pins the other half: the
//! finalization hook never hands `claim` a staging failure. Production is set
//! up one failure short of nothing — it has already failed often enough to
//! page — so a staging failure that reached `claim` would take the slot.

use chrono::{Duration, Utc};
use entity::app_function_invocations;
use oxy_app::server::api::custom_apps_functions::failure_alert::{
    FailureKey, THRESHOLD, Verdict, claim,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue::Set, ConnectionTrait, DatabaseBackend, DatabaseConnection,
    Statement,
};
use serde_json::json;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{FunctionSpec, invocations, publish_app, seeded_tenant};
use crate::staging_functions::{call_on, make_guest_staff, staging_host};

const FUNCTION: &str = "boom";

fn boom() -> Vec<FunctionSpec> {
    vec![FunctionSpec {
        name: FUNCTION,
        manifest: json!({ "route": true }),
        js: r#"export default async () => { throw new Error("ledger sync failed"); };"#,
    }]
}

/// The ops Slack target `failure_page::observe` reads. Nothing is posted
/// unless a page is claimed, which this test asserts never happens.
fn configure_pager() {
    unsafe {
        std::env::set_var("OXY_OPS_SLACK_BOT_TOKEN", "xoxb-staging-pager-test");
        std::env::set_var("OXY_OPS_SLACK_CHANNEL", "C0STAGINGTEST");
    }
}

async fn claims(db: &DatabaseConnection, app_id: Uuid) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(
        DatabaseBackend::Postgres,
        "SELECT count(*) AS n FROM app_function_failure_alerts WHERE app_id = $1",
        [app_id.into()],
    ))
    .await
    .expect("count claims")
    .expect("a row")
    .try_get::<i64>("", "n")
    .expect("n")
}

/// A production failure of `fingerprint`, `ago` in the past.
async fn failed_in_production(
    db: &DatabaseConnection,
    app_id: Uuid,
    build_id: Uuid,
    fingerprint: &str,
    ago: Duration,
) -> Uuid {
    let id = Uuid::new_v4();
    app_function_invocations::ActiveModel {
        id: Set(id),
        app_id: Set(app_id),
        build_id: Set(build_id),
        function_name: Set(FUNCTION.into()),
        mode: Set("route".into()),
        user_id: Set(None),
        status: Set("error".into()),
        duration_ms: Set(Some(12)),
        error: Set(Some("function threw: Error: ledger sync failed".into())),
        cancel_requested_at: Set(None),
        created_at: Set((Utc::now() - ago).into()),
        idempotency_key: Set(None),
        result_body: Set(None),
        result_status: Set(None),
        request_hash: Set(None),
        failure_fingerprint: Set(Some(fingerprint.into())),
        environment: Set("production".into()),
    }
    .insert(db)
    .await
    .expect("seed a production failure");
    id
}

#[tokio::test]
async fn a_failing_staging_function_neither_pages_nor_claims_productions_slot() {
    // The demo workspace is the Local org's; the per-test database keeps the
    // claim rows this test counts to its own app.
    let t = seeded_tenant().await;
    let app = "stg-pager";
    let published = publish_app(&t, app, demo_workspace_id(), &boom()).await;
    configure_pager();
    make_guest_staff();

    // The first staging failure names the fingerprint every environment
    // shares for this error.
    call_on(&t, app, FUNCTION, &staging_host(&t, app), &[]).await;
    let first = invocations(&t.db, published.app_id, FUNCTION).await;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].environment, "staging");
    assert_eq!(first[0].status, "error");
    let fingerprint = first[0]
        .failure_fingerprint
        .clone()
        .expect("a failed staging invocation is fingerprinted like any other");

    // Production has failed this way often enough to page.
    let mut production = Uuid::nil();
    for minutes in 1..=THRESHOLD {
        production = failed_in_production(
            &t.db,
            published.app_id,
            first[0].build_id,
            &fingerprint,
            Duration::minutes(minutes),
        )
        .await;
    }

    // Staging fails again, the same way.
    call_on(&t, app, FUNCTION, &staging_host(&t, app), &[]).await;
    assert_eq!(
        claims(&t.db, published.app_id).await,
        0,
        "a staging failure claimed (and would have paged) production's slot"
    );

    // Production's slot is still its own: its next evaluation pages.
    let key = FailureKey {
        app_id: published.app_id,
        function_name: FUNCTION,
        fingerprint: &fingerprint,
    };
    let verdict = claim(
        &t.db,
        production,
        key,
        Utc::now(),
        Utc::now() - Duration::days(30),
    )
    .await
    .expect("claim");
    assert!(
        matches!(verdict, Verdict::Page { .. }),
        "production pages as if staging had never failed: {verdict:?}"
    );
}
