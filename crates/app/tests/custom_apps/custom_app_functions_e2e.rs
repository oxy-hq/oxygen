//! Oxy Functions end to end through a published app: publish with promote, a
//! route call run by the real V8 isolate, and a function that throws.
//!
//! Every layer under this had its own tests and the gap sat between them. The
//! isolate's tests run hand-written JS against a `MockHost`; the engine tests
//! call `ProjectFunctionHost` from Rust; the publish test only checks a refusal;
//! the pager tests insert invocation rows by hand. Nothing published an app with
//! functions and ran one, which is where the ClickHouse `Code: 27` outage lived.
//!
//! These publish through `publish()`, call through `serve_dispatch`, run the
//! bundled module in the isolate, and read back what finalization wrote — see
//! `custom_app_functions_fixture`. Each asserts a value only the JS could have
//! computed, so a green run means the isolate ran, not merely that a status
//! code came back.
//!
//! Needs Postgres (a testcontainer, or `OXY_DATABASE_URL`). The ClickHouse write
//! is `custom_app_functions_clickhouse`; the admin run is
//! `custom_app_functions_manual_run`.

use axum::http::StatusCode;
use entity::{app_builds, app_functions, apps};
use oxy_app::server::api::custom_apps_build_store::{build_prefix, get_object};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter};
use serde_json::json;
use uuid::Uuid;

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    BUILD_ID, FunctionSpec, call_function, invocations, publish_app, seeded_tenant,
};

const APP: &str = "fn-e2e";

/// Prices an order: a total and a SKU list the Rust side never computes.
const PRICE_ORDER_JS: &str = r#"
export default async (req, ctx) => {
  const { lines } = JSON.parse(req.body);
  const totalCents = lines.reduce((sum, line) => sum + line.qty * line.unitCents, 0);
  const skus = lines.map((line) => line.sku.toUpperCase()).sort().join("+");
  return Response.json({ totalCents, skus, caller: ctx.user.id });
};
"#;

const CLOSE_LEDGER_JS: &str = r#"
export default async (req) => {
  const { ledger } = JSON.parse(req.body);
  throw new Error(`ledger ${ledger} does not balance`);
};
"#;

fn functions() -> Vec<FunctionSpec> {
    vec![
        FunctionSpec {
            name: "price-order",
            manifest: json!({ "route": true }),
            js: PRICE_ORDER_JS,
        },
        FunctionSpec {
            name: "close-ledger",
            manifest: json!({ "route": true }),
            js: CLOSE_LEDGER_JS,
        },
    ]
}

#[tokio::test]
async fn publishing_with_promote_makes_the_build_live_and_stores_its_function_artifacts() {
    let t = seeded_tenant().await;
    let published = publish_app(&t, APP, demo_workspace_id(), &functions()).await;

    let app = apps::Entity::find_by_id(published.app_id)
        .one(&t.db)
        .await
        .expect("query app")
        .expect("publish created the app");
    let build = app_builds::Entity::find()
        .filter(app_builds::Column::AppId.eq(app.id))
        .filter(app_builds::Column::BuildId.eq(BUILD_ID))
        .one(&t.db)
        .await
        .expect("query build")
        .expect("publish recorded the build");
    assert_eq!(
        app.published_build_id,
        Some(build.id),
        "promote must point the live channel at the new build"
    );
    assert!(
        app.published_at.is_some(),
        "a promoted app without published_at is hidden from the launcher"
    );

    assert_function_artifacts(&t.db, app.id, build.id).await;
}

/// The build's `app_functions` rows point at `functions/<name>.js` under its
/// build prefix, and the build store holds each module byte for byte.
async fn assert_function_artifacts(db: &DatabaseConnection, app_id: Uuid, build_pk: Uuid) {
    let mut recorded: Vec<(String, String)> = app_functions::Entity::find()
        .filter(app_functions::Column::BuildId.eq(build_pk))
        .all(db)
        .await
        .expect("query app_functions")
        .into_iter()
        .map(|f| (f.name, f.artifact_key))
        .collect();
    recorded.sort();
    let prefix = build_prefix(app_id, BUILD_ID);
    assert_eq!(
        recorded,
        vec![
            (
                "close-ledger".to_string(),
                format!("{prefix}functions/close-ledger.js")
            ),
            (
                "price-order".to_string(),
                format!("{prefix}functions/price-order.js")
            ),
        ]
    );
    for f in functions() {
        let path = format!("functions/{}.js", f.name);
        let stored = get_object(app_id, BUILD_ID, &path)
            .await
            .expect("read the build store")
            .unwrap_or_else(|| panic!("{path} is not in the build store"));
        assert_eq!(
            stored.as_ref(),
            f.js.as_bytes(),
            "{path} was stored altered"
        );
    }
}

#[tokio::test]
async fn a_route_function_runs_in_the_isolate_and_records_its_invocation() {
    let t = seeded_tenant().await;
    let published = publish_app(&t, APP, demo_workspace_id(), &functions()).await;

    let call = call_function(
        APP,
        "price-order",
        json!({ "lines": [
            { "sku": "cd-2", "qty": 2, "unitCents": 1125 },
            { "sku": "ab-1", "qty": 3, "unitCents": 250 },
        ] }),
    )
    .await;

    assert_eq!(call.status, StatusCode::OK, "stream: {}", call.raw);
    assert_eq!(
        call.frame("data"),
        Some(&json!({ "totalCents": 3000, "skus": "AB-1+CD-2", "caller": t.guest_id.to_string() })),
        "the isolate's computed result, as the SDK receives it; stream: {}",
        call.raw
    );
    assert_eq!(call.frame("done"), Some(&json!({ "status": 200 })));

    let rows = invocations(&t.db, published.app_id, "price-order").await;
    let seen: Vec<_> = rows
        .iter()
        .map(|r| {
            (
                r.mode.as_str(),
                r.status.as_str(),
                r.user_id,
                r.error.as_deref(),
            )
        })
        .collect();
    assert_eq!(seen, vec![("route", "success", Some(t.guest_id), None)]);
    assert_eq!(rows[0].failure_fingerprint, None, "a success pages nobody");
}

#[tokio::test]
async fn a_function_that_throws_records_an_error_invocation_with_a_failure_fingerprint() {
    let t = seeded_tenant().await;
    let published = publish_app(&t, APP, demo_workspace_id(), &functions()).await;

    let call = call_function(APP, "close-ledger", json!({ "ledger": 42 })).await;

    let error = call
        .frame("error")
        .unwrap_or_else(|| panic!("a throw must end the stream with `error`: {}", call.raw));
    assert_eq!(error["error"], "error");
    let message = error["message"].as_str().expect("error message");
    assert!(
        message.contains("ledger 42 does not balance"),
        "the author's own message reaches the caller: {message}"
    );

    let rows = invocations(&t.db, published.app_id, "close-ledger").await;
    let seen: Vec<_> = rows
        .iter()
        .map(|r| (r.mode.as_str(), r.status.as_str()))
        .collect();
    assert_eq!(seen, vec![("route", "error")]);
    let row = &rows[0];
    assert!(
        row.error
            .as_deref()
            .is_some_and(|e| e.contains("ledger 42 does not balance")),
        "the invocation keeps the thrown message: {:?}",
        row.error
    );
    let fingerprint = row
        .failure_fingerprint
        .as_deref()
        .expect("finalization writes the fingerprint the pager groups failures by");
    assert!(
        !fingerprint.is_empty(),
        "an empty fingerprint groups nothing"
    );
}
