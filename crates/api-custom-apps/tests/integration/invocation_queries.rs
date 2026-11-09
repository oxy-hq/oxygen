//! How many SQL statements a warm function invocation issues before the
//! isolate starts.
//!
//! Each statement is a round trip to Postgres on a hot path, so the count is
//! pinned as an upper bound: a new per-request read added to admission makes
//! this test fail, and whoever adds it either removes another or raises the
//! bound on purpose. The bound is a count of statements, never a duration.
//!
//! The statements are counted from `sqlx::query` tracing events, which sqlx
//! emits when the process's tracing subscriber accepts `DEBUG`
//! (`sqlx_logging_enabled` in `crates/platform/src/db/client.rs` reads the
//! level when the pool is created). The subscriber is installed before the
//! first connection is made, so it must be the first thing a test here does;
//! nextest runs each test in its own process, which makes that hold.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use entity::{org_members, partner_grants};
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::{Layer, Registry};

use crate::common::demo_workspace_id;
use crate::custom_app_functions_fixture::{
    FunctionSpec, Tenant, call_function, publish_app, seeded_tenant,
};

const APP: &str = "invocation-queries";

/// What a warm call may issue before its isolate starts, for the org owner and
/// for a plain member. One of each bound is the shared rate limiter's upsert
/// (`oxy_app_core::rate_limit::take`, one `INSERT … ON CONFLICT` per call):
/// added on purpose in 2026-11 so the per-function limit is one number across
/// replicas rather than one per pod. The owner is an app admin, a role that is never
/// remembered, so the owner's standing is read on every call; a member holds no
/// role, and that is remembered for 60 s (the second call here is warm for it).
const MAX_OWNER: usize = 12;
const MAX_MEMBER: usize = 7;
/// A member of an org that holds a partner grant: their "no role" is never
/// remembered (see `resolve_app_role`), so they pay the standing reads, but the
/// check that decides this is itself remembered and adds nothing per call.
const MAX_PARTNER_ORG_MEMBER: usize = 15;

const NOOP_JS: &str = r#"
export default async () => Response.json({ ok: true });
"#;

static COUNTING: AtomicBool = AtomicBool::new(false);
static COUNT: AtomicUsize = AtomicUsize::new(0);
static STATEMENTS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

fn statements() -> &'static Mutex<Vec<String>> {
    STATEMENTS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Records the text of every `sqlx::query` event while [`COUNTING`] is on.
struct StatementCounter;

struct StatementText(String);

impl StatementText {
    fn wants(&self, field: &Field) -> bool {
        field.name() == "db.statement" || (self.0.is_empty() && field.name() == "summary")
    }
}

impl Visit for StatementText {
    fn record_str(&mut self, field: &Field, value: &str) {
        if self.wants(field) {
            self.0 = value.to_string();
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if self.wants(field) {
            self.0 = format!("{value:?}");
        }
    }
}

impl<S: Subscriber> Layer<S> for StatementCounter {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "sqlx::query" || !COUNTING.load(Ordering::SeqCst) {
            return;
        }
        let mut text = StatementText(String::new());
        event.record(&mut text);
        COUNT.fetch_add(1, Ordering::SeqCst);
        let one_line = text.0.split_whitespace().collect::<Vec<_>>().join(" ");
        statements()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(one_line);
    }
}

fn install_counter() {
    let subscriber = Registry::default()
        .with(tracing_subscriber::filter::LevelFilter::DEBUG)
        .with(StatementCounter);
    tracing::subscriber::set_global_default(subscriber)
        .expect("the statement counter must be the process's first subscriber");
}

/// The statements one call issues before its isolate starts. The function
/// makes no host calls, so everything up to the update that settles its
/// invocation row is admission and isolate setup.
async fn statements_before_isolate() -> Vec<String> {
    COUNT.store(0, Ordering::SeqCst);
    statements().lock().unwrap().clear();
    COUNTING.store(true, Ordering::SeqCst);
    let call = call_function(APP, "noop", json!({})).await;
    COUNTING.store(false, Ordering::SeqCst);
    assert_eq!(call.status, 200, "{}", call.raw);
    // The live-tail watcher poll runs on its own timer, not for this call; it
    // lands in the window on a slow machine and is not admission. Nor is the
    // previous call's call-trace write, which is spawned off that call's
    // request path (`call_trace_store::spawn_write`) and can land here.
    let seen: Vec<String> = statements()
        .lock()
        .unwrap()
        .iter()
        .filter(|s| !s.contains("app_environment_tails"))
        .filter(|s| !s.contains("app_function_invocation_calls"))
        // The rate limiter's expired-window cleanup is spawned off the request
        // path every 64th take (and on the first take of a fresh process, which
        // every nextest process is), so it too can land in the window.
        .filter(|s| !(s.contains("DELETE") && s.contains("rate_limit_windows")))
        .cloned()
        .collect();
    let settled = seen
        .iter()
        .position(|s| {
            s.to_ascii_uppercase()
                .contains("UPDATE \"APP_FUNCTION_INVOCATIONS\"")
        })
        .unwrap_or(seen.len());
    seen[..settled].to_vec()
}

fn report(who: &str, before_isolate: &[String], bound: usize) {
    for (i, s) in before_isolate.iter().enumerate() {
        eprintln!("{who} statement {:>2}: {s}", i + 1);
    }
    eprintln!(
        "{who}: {} statements before the isolate",
        before_isolate.len()
    );
    assert!(
        before_isolate.len() <= bound,
        "a warm call by {who} issued {} statements before the isolate (bound {bound}):\n{}",
        before_isolate.len(),
        before_isolate.join("\n")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_warm_function_call_reaches_the_isolate_in_few_queries() {
    install_counter();
    let t = seeded_tenant().await;
    let functions = vec![FunctionSpec {
        name: "noop",
        manifest: json!({ "route": true }),
        js: NOOP_JS,
    }];
    publish_app(&t, APP, demo_workspace_id(), &functions).await;

    // The first call fills every cache the next one reads.
    let cold = call_function(APP, "noop", json!({})).await;
    assert_eq!(cold.status, 200, "{}", cold.raw);
    report(
        "the org owner",
        &statements_before_isolate().await,
        MAX_OWNER,
    );

    // Most callers are plain members, not officers.
    demote_guest_to_member(&t).await;
    let first = call_function(APP, "noop", json!({})).await;
    assert_eq!(first.status, 200, "{}", first.raw);
    report("a member", &statements_before_isolate().await, MAX_MEMBER);

    // The same member once their org holds a partner grant.
    partner_grants::ActiveModel {
        org_id: sea_orm::ActiveValue::Set(t.org_id),
        status: sea_orm::ActiveValue::Set("active".into()),
        created_by: sea_orm::ActiveValue::Set(None),
        created_at: sea_orm::ActiveValue::Set(chrono::Utc::now().into()),
    }
    .insert(&t.db)
    .await
    .expect("seed a partner grant");
    oxy_api_custom_apps::custom_apps_cache::clear_access_caches();
    let warm = call_function(APP, "noop", json!({})).await;
    assert_eq!(warm.status, 200, "{}", warm.raw);
    report(
        "a member of a partner org",
        &statements_before_isolate().await,
        MAX_PARTNER_ORG_MEMBER,
    );
}

async fn demote_guest_to_member(t: &Tenant) {
    let row = org_members::Entity::find()
        .filter(org_members::Column::OrgId.eq(t.org_id))
        .filter(org_members::Column::UserId.eq(t.guest_id))
        .one(&t.db)
        .await
        .expect("query membership")
        .expect("the guest is a member");
    let mut row: org_members::ActiveModel = row.into();
    row.role = sea_orm::ActiveValue::Set(org_members::OrgRole::Member);
    row.update(&t.db).await.expect("demote the guest");
}
