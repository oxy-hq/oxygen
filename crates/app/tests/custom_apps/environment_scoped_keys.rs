//! The per-call records carry the app environment: the idempotency record,
//! the result cache and the migration ledger
//! (`internal-docs/2026-09-10-custom-app-environments-design.md` §3.4, §4.3,
//! §8). The rate limit is unit-tested beside its code in
//! `custom_apps_functions::rate_limit`.
//!
//! Each test sets up the collision the key exists to prevent and shows that
//! production is not affected by it:
//!
//! - an `Idempotency-Key` spent by a real staging call does not replay into
//!   production, so the production write runs;
//! - a result cached by a real staging call is never served to production,
//!   though both environments serve the same build;
//! - a migration applied to a non-production target does not read as applied
//!   to production, so a promote does not skip production's DDL. Nothing can
//!   yet *make* a staging apply, so that test writes the ledger row itself.

use axum::http::StatusCode;
use chrono::Utc;
use entity::custom_app_migrations;
use oxy_app::server::api::custom_apps_migrations::{MigrationTarget, read_ledger};
use sea_orm::{ActiveModelTrait, ActiveValue::Set, EntityTrait};
use serde_json::{Value, json};

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    FnCall, FunctionSpec, ORG_SLUG, call_function_with, invocations, publish_app, seeded_tenant,
};
use crate::staging_functions::make_guest_staff;

const APP: &str = "env-keys";

/// A write whose result differs every time it actually runs, so a replay is
/// told apart from a run by the value alone. `ctx.channel` says where it ran.
const STAMP_JS: &str = r#"
export default async (req, ctx) => Response.json({ nonce: Math.random(), channel: ctx.channel });
"#;

fn stamp(manifest: Value) -> Vec<FunctionSpec> {
    vec![FunctionSpec {
        name: "stamp",
        manifest,
        js: STAMP_JS,
    }]
}

fn host(environment: &str) -> String {
    match environment {
        "production" => format!("{ORG_SLUG}--{APP}.customer-apps.oxygen-hq.com"),
        env => format!("{env}--{ORG_SLUG}--{APP}.customer-apps.oxygen-hq.com"),
    }
}

async fn call_in(environment: &str, extra: &[(&str, &str)]) -> FnCall {
    let host = host(environment);
    let mut headers = vec![("host", host.as_str())];
    headers.extend_from_slice(extra);
    call_function_with(ORG_SLUG, APP, "stamp", json!({ "order": 17 }), &headers).await
}

fn stamped(call: &FnCall) -> (Value, Value) {
    assert_eq!(call.status, StatusCode::OK, "stream: {}", call.raw);
    let data = call
        .frame("data")
        .unwrap_or_else(|| panic!("no result: {}", call.raw));
    (data["nonce"].clone(), data["channel"].clone())
}

#[tokio::test]
async fn a_key_spent_in_staging_does_not_replay_into_production() {
    let t = seeded_tenant().await;
    // Promoted, so staging and production serve the same build: only the
    // environment in the key tells their records apart.
    let published = publish_app(
        &t,
        APP,
        demo_workspace_id(),
        &stamp(json!({ "route": true })),
    )
    .await;
    make_guest_staff();
    let key = [("idempotency-key", "order-17")];

    let (staging_nonce, channel) = stamped(&call_in("staging", &key).await);
    assert_eq!(channel, "staging");

    // Production, same key, same body: the write must run, not replay
    // staging's stored result.
    let (production_nonce, channel) = stamped(&call_in("production", &key).await);
    assert_eq!(channel, "production");
    assert_ne!(
        production_nonce, staging_nonce,
        "production replayed the result staging stored under the same key — the production \
         write was skipped"
    );
    let rows = invocations(&t.db, published.app_id, "stamp").await;
    let seen: Vec<(&str, &str)> = rows
        .iter()
        .map(|r| (r.environment.as_str(), r.status.as_str()))
        .collect();
    assert_eq!(
        seen,
        vec![("staging", "success"), ("production", "success")],
        "each environment ran under its own record"
    );

    // Each environment's own idempotency is unchanged: the same key replays
    // that environment's result and runs nothing.
    assert_eq!(
        stamped(&call_in("production", &key).await).0,
        production_nonce
    );
    assert_eq!(stamped(&call_in("staging", &key).await).0, staging_nonce);
    assert_eq!(
        invocations(&t.db, published.app_id, "stamp").await.len(),
        2,
        "a retry with a spent key replays; it does not run again"
    );
}

#[tokio::test]
async fn a_result_cached_in_staging_is_never_served_to_production() {
    let t = seeded_tenant().await;
    let cached = json!({ "route": true, "cache": { "ttlSeconds": 300 } });
    publish_app(&t, APP, demo_workspace_id(), &stamp(cached)).await;
    make_guest_staff();

    let (staging_nonce, channel) = stamped(&call_in("staging", &[]).await);
    assert_eq!(channel, "staging");
    let (production_nonce, channel) = stamped(&call_in("production", &[]).await);
    assert_eq!(
        channel, "production",
        "production was served staging's cached result"
    );
    assert_ne!(production_nonce, staging_nonce);

    // Each environment's cache still works for itself.
    assert_eq!(
        stamped(&call_in("production", &[]).await).0,
        production_nonce
    );
    assert_eq!(stamped(&call_in("staging", &[]).await).0, staging_nonce);
}

async fn ledger_row(
    db: &sea_orm::DatabaseConnection,
    app_id: uuid::Uuid,
    target: &MigrationTarget,
    checksum: &str,
) {
    custom_app_migrations::ActiveModel {
        app_id: Set(app_id),
        store: Set("oltp".to_string()),
        target: Set(target.as_key()),
        filename: Set("0002_add_column.sql".to_string()),
        checksum: Set(checksum.to_string()),
        applied_at: Set(Utc::now().fixed_offset()),
        applied_by_build: Set(None),
    }
    .insert(db)
    .await
    .expect("record a ledger row");
}

#[tokio::test]
async fn a_migration_applied_to_staging_is_not_applied_to_production() {
    let t = seeded_tenant().await;
    let app_id = publish_app(
        &t,
        APP,
        demo_workspace_id(),
        &stamp(json!({ "route": true })),
    )
    .await
    .app_id;
    let staging = MigrationTarget::Branch("br-staging".to_string());

    ledger_row(&t.db, app_id, &staging, "abc").await;

    let production = read_ledger(&t.db, app_id, "oltp", &MigrationTarget::Production)
        .await
        .expect("read the production ledger");
    assert!(
        production.is_empty(),
        "staging's applied file reads as applied to production, so the promote would skip \
         production's DDL: {production:?}"
    );
    let on_staging = read_ledger(&t.db, app_id, "oltp", &staging)
        .await
        .expect("read the staging ledger");
    assert_eq!(
        on_staging.get("0002_add_column.sql").map(String::as_str),
        Some("abc")
    );

    // Production then applies the same file: the key admits both records, and
    // each target reads its own.
    ledger_row(&t.db, app_id, &MigrationTarget::Production, "abc").await;
    let production = read_ledger(&t.db, app_id, "oltp", &MigrationTarget::Production)
        .await
        .expect("read the production ledger");
    assert_eq!(production.len(), 1);
    assert_eq!(
        custom_app_migrations::Entity::find()
            .all(&t.db)
            .await
            .expect("read the ledger")
            .len(),
        2
    );
}
