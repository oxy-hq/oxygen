//! `GET /api/customer-apps/function-failures` — the feed ferret's triage
//! reads to weigh an alert: which apps failed, whose, and with what error.
//!
//! **ferret depends on this; changing it needs a matching change in
//! oxy-hq/infrastructure `services/ferret`** (`failureRow` / `failurePage`,
//! `failureParams`, `walkFailures` in `src/uptime/staff-client.ts`;
//! `src/tools/function-failures.ts`; `src/triage/function-source.ts`).
//!
//! ferret asks for `since`/`until` as `Date.toISOString()` (milliseconds,
//! `Z`), a window of up to 7 days, `limit=500`, optionally `org_id` / `app_id`,
//! then follows `next_cursor` until it is null. A `400` on any of that — a
//! tighter window, a lower page cap — is UNKNOWN, never "no failures".

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use entity::{app_builds, app_function_invocations, apps};
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection};
use serde_json::Value;
use uuid::Uuid;

use super::super::stack::published_app;
use super::{Field, assert_fields, ferret_get, query, staff_fixture, staff_keys};

const FAILURE_ROW: &[(&str, Field)] = &[
    ("invocation_id", Field::Str),
    ("invoked_at", Field::Str),
    ("org_id", Field::Str),
    ("org_slug", Field::OptStr),
    ("org_name", Field::OptStr),
    ("org_billing_status", Field::OptStr),
    ("app_id", Field::Str),
    ("app_slug", Field::Str),
    ("function_name", Field::Str),
    ("mode", Field::OptStr),
    ("status", Field::OptStr),
    ("kind", Field::OptStr),
    ("fingerprint", Field::OptStr),
    ("error", Field::OptStr),
    ("duration_ms", Field::OptNum),
    ("build_uuid", Field::OptStr),
];

const FAILURE_PAGE: &[(&str, Field)] = &[
    ("failures", Field::Array),
    ("next_cursor", Field::OptStr),
    ("since", Field::OptStr),
    ("until", Field::OptStr),
];

/// A timestamp as JavaScript's `Date.toISOString()` writes it.
fn iso(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Whole seconds: a Postgres round trip of `Utc::now()` is not exact on Linux.
fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(Utc::now().timestamp(), 0).unwrap()
}

/// A build row for `app`, which every invocation row names.
async fn build_of(db: &DatabaseConnection, app: &apps::Model) -> Uuid {
    let id = Uuid::new_v4();
    app_builds::ActiveModel {
        id: ActiveValue::Set(id),
        app_id: ActiveValue::Set(app.id),
        build_id: ActiveValue::Set("contract-b1".into()),
        s3_prefix: ActiveValue::Set(format!("customer-apps/{}/builds/contract-b1/", app.id)),
        created_at: ActiveValue::Set(Utc::now().into()),
        validation_status: ActiveValue::Set("passed".into()),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed build");
    id
}

/// One failed production invocation of `app` at `at`.
async fn failed(db: &DatabaseConnection, app: &apps::Model, build: Uuid, at: DateTime<Utc>) {
    app_function_invocations::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        app_id: ActiveValue::Set(app.id),
        build_id: ActiveValue::Set(build),
        function_name: ActiveValue::Set("post-je".into()),
        mode: ActiveValue::Set("route".into()),
        user_id: ActiveValue::Set(None),
        status: ActiveValue::Set("error".into()),
        duration_ms: ActiveValue::Set(Some(40)),
        error: ActiveValue::Set(Some("function threw: HostError: ctx.warehouse".into())),
        cancel_requested_at: ActiveValue::Set(None),
        created_at: ActiveValue::Set(at.into()),
        idempotency_key: ActiveValue::Set(None),
        result_body: ActiveValue::Set(None),
        result_status: ActiveValue::Set(None),
        request_hash: ActiveValue::Set(None),
        failure_fingerprint: ActiveValue::Set(Some("bace6ddec50847c8".into())),
        environment: ActiveValue::Set("production".into()),
        credential_token_id: ActiveValue::Set(None),
        error_kind: ActiveValue::Set(Some("threw".into())),
    }
    .insert(db)
    .await
    .expect("seed invocation");
}

/// The feed's URI for ferret's first page over the last `days`, plus `extra`.
fn feed_uri(until: DateTime<Utc>, days: i64, limit: &str, extra: &[(&str, &str)]) -> String {
    let (since, until) = (iso(until - Duration::days(days)), iso(until));
    let mut pairs = vec![
        ("since", since.as_str()),
        ("until", until.as_str()),
        ("limit", limit),
    ];
    pairs.extend_from_slice(extra);
    format!("/customer-apps/function-failures?{}", query(&pairs))
}

#[tokio::test]
async fn the_feed_answers_ferret_s_query_with_the_page_it_parses() {
    let fx = staff_fixture().await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let build = build_of(&fx.db, &app).await;
    let t = now();
    failed(&fx.db, &app, build, t - Duration::minutes(5)).await;
    let (org, app_id) = (fx.org_id.to_string(), app.id.to_string());

    for (shape, key) in staff_keys(&fx).await {
        // The widest query ferret sends: 7 days, its page of 500, both filters.
        let extra = [("org_id", org.as_str()), ("app_id", app_id.as_str())];
        let page = ferret_get(Some(&key), &feed_uri(t, 7, "500", &extra)).await;
        page.assert_json_200(&format!("the failure feed, {shape}"));
        assert_fields(&page.body, FAILURE_PAGE, "the failure page");
        let rows = page.body["failures"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "{}", page.body);
        assert_fields(&rows[0], FAILURE_ROW, "a failure row");
        assert_eq!(rows[0]["app_id"], app_id.as_str());
        assert_eq!(
            rows[0]["fingerprint"], "bace6ddec50847c8",
            "triage groups by it"
        );
        assert!(page.body["next_cursor"].is_null(), "one row, one page");
    }
}

#[tokio::test]
async fn next_cursor_walks_every_page_and_then_says_null() {
    let fx = staff_fixture().await;
    let app = published_app(&fx.db, fx.org_id, fx.workspace_id).await;
    let build = build_of(&fx.db, &app).await;
    let t = now();
    failed(&fx.db, &app, build, t - Duration::minutes(10)).await;
    failed(&fx.db, &app, build, t - Duration::minutes(5)).await;
    let (_, key) = staff_keys(&fx).await.remove(1);

    let first = ferret_get(Some(&key), &feed_uri(t, 1, "1", &[])).await;
    first.assert_json_200("the first feed page");
    let cursor = first.body["next_cursor"]
        .as_str()
        .unwrap_or_else(|| panic!("a full page names the next: {}", first.body))
        .to_string();
    let second = ferret_get(Some(&key), &feed_uri(t, 1, "1", &[("cursor", &cursor)])).await;
    second.assert_json_200("the second feed page");
    let ids = |p: &Value| p["failures"][0]["invocation_id"].clone();
    assert_ne!(ids(&first.body), ids(&second.body), "the cursor moved");
    assert!(
        second.body["next_cursor"].is_null(),
        "the last page ends ferret's walk: {}",
        second.body
    );
}
