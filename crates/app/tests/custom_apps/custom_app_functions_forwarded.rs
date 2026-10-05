//! The forward-to-Factory arm of `server::invocation_placement`, over a real
//! hop.
//!
//! `custom_app_functions_diskless` proves what a replica answers when it holds
//! no working copy and no Factory is reachable: two refusals. In production a
//! Factory is reachable — `OXY_IDE_UPSTREAM` names the ide Service — and a
//! workspace a replica cannot serve is replayed there instead. That hop was
//! covered by unit tests of the decision and of the rebuilt request only. Here
//! it runs: this process is the `serve` replica (no working copy,
//! `OXY_IDE_UPSTREAM` set), and the Factory is a server bound in this process
//! that records every request reaching it and answers with a marker nothing on
//! the replica's side produces.
//!
//! The Factory is a recorder rather than a second copy of the real handler
//! because the working-copy flag (`workspace_fs_probe`) is per process, as
//! `OXY_ROLE` is. The real handler in this process would read the same flag,
//! find the loop-guard header on the replayed request and refuse it — what a
//! serve pod does when `OXY_IDE_UPSTREAM` points at the fleet by mistake, not
//! what the ide does. The real handler in the Factory's position (flag set,
//! working copy present) is every other `custom_app_functions_*` test; this
//! one proves the hop between the two.
//!
//! One process, one Factory, one replica, three apps — the three workspaces
//! `invocation_placement` distinguishes, in one process because
//! `ide_upstream()` is read once per process:
//!
//! - **servable** (Postgres, compiled): runs here, and the Factory sees
//!   nothing — a reachable Factory does not pull a call it is not needed for;
//! - **a database that is a file in the checkout** (local DuckDB, no S3
//!   mirror): replayed. The Factory receives the request that arrived here —
//!   method, path, body, content-type, the caller's `Idempotency-Key` — plus
//!   the loop guard, and its answer is what the caller gets, headers included;
//!   no compile is queued and no invocation row is written;
//! - **nothing compiled**: replayed the same way, and a compile is queued so
//!   the workspace stops needing the Factory.
//!
//! A relayed answer carries the pair an operator reads a hop from —
//! `x-oxy-forwarded-via: <this process>` and the Factory's own
//! `x-oxy-served-by` — and the local answer carries neither.
//!
//! The second test is the production case the first cannot show: the Factory
//! configured but down (stopped, restarting), `OXY_IDE_UPSTREAM` naming a
//! closed port. A replica must then answer the same 503s it gives with no
//! Factory at all — `WorkspaceNotCompiled` with the recompile header and a
//! queued compile, `WorkspaceNeedsWorkingCopy` naming the ide — not the
//! generic ide-down 502, which carries neither. A separate test because
//! `ide_upstream()` is read once per process, and nextest runs each test in
//! its own.

use axum::http::StatusCode;
use oxy::workspace_fs_probe::leaks;
use serde_json::{Value, json};
use wiremock::matchers::{method, path_regex};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::custom_app_functions_diskless::{
    AsDisklessReplica, READS, functions, queued_compiles, uncompiled_workspace, write_view,
};
use crate::custom_app_functions_fixture::{
    FnCall, call_function_in, call_function_with, invocations, publish_app, seeded_tenant,
    throwaway_org,
};
use crate::custom_app_functions_shape_zoo::{compile, write_config};
use crate::warehouse_writes_on_engines::postgres_entry;

/// What the Factory stamps on its own answers (`role_middleware::stamp` on the
/// ide), and the body it returns here. Neither can come from this side.
const FACTORY_SERVED_BY: &str = "ide@factory#1";
const FACTORY_DONE: &str = "event: done\ndata: {\"status\":200,\"via\":\"factory\"}\n\n";

/// The Factory: bound in this process, recording what reaches it, answering
/// every `/fn/` call with the marker. `OXY_IDE_UPSTREAM` points at it before
/// anything reads `ide_upstream()`, which caches once per process.
async fn start_factory() -> MockServer {
    let factory = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path_regex(r"^/customer-apps/[^/]+/[^/]+/fn/[^/]+$"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-oxy-served-by", FACTORY_SERVED_BY)
                .set_body_raw(FACTORY_DONE, "text/event-stream"),
        )
        .mount(&factory)
        .await;
    // SAFETY: before anything else in this test reads the environment, and
    // nextest gives each test its own process, so it reaches no other test.
    unsafe { std::env::set_var("OXY_IDE_UPSTREAM", factory.uri()) };
    factory
}

/// A Factory that is configured but not answering: a port bound and released,
/// so a connect is refused at once. `OXY_IDE_UPSTREAM` names it before
/// anything reads `ide_upstream()`.
fn point_at_a_down_factory() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a port");
    let port = listener.local_addr().expect("its address").port();
    drop(listener);
    // SAFETY: as in `start_factory`.
    unsafe { std::env::set_var("OXY_IDE_UPSTREAM", format!("http://127.0.0.1:{port}")) };
}

/// Everything the Factory has received, oldest first.
async fn arrived_at(factory: &MockServer) -> Vec<wiremock::Request> {
    factory.received_requests().await.unwrap_or_default()
}

fn header<'a>(headers: &'a axum::http::HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|v| v.to_str().ok())
}

/// The Factory's answer as the caller sees it: its status and body verbatim,
/// its own `served-by`, and this process named as the relay.
fn assert_factory_answered(call: &FnCall) {
    assert_eq!(
        call.status,
        StatusCode::OK,
        "the Factory's status, verbatim: {}",
        call.raw
    );
    assert_eq!(
        call.frame("done"),
        Some(&json!({ "status": 200, "via": "factory" })),
        "the Factory's body, verbatim: {}",
        call.raw
    );
    assert_eq!(
        header(&call.headers, "x-oxy-served-by"),
        Some(FACTORY_SERVED_BY),
        "the Factory's own served-by crosses back: {:?}",
        call.headers
    );
    let via = header(&call.headers, "x-oxy-forwarded-via")
        .unwrap_or_else(|| panic!("a relayed answer names its relay: {:?}", call.headers));
    assert!(
        via.ends_with(&format!("#{}", std::process::id())),
        "the relay is this process: {via}"
    );
}

/// What reached the Factory is the request that arrived here, plus the loop
/// guard that stops the Factory forwarding it again.
fn assert_replayed(seen: &wiremock::Request, path: &str, body: &Value) {
    assert_eq!(seen.method.as_str(), "POST");
    assert_eq!(
        seen.url.path(),
        path,
        "the URI the outer stack routed on, verbatim"
    );
    assert_eq!(
        header(&seen.headers, "x-oxy-forwarded-by"),
        Some("serve"),
        "the loop guard: {:?}",
        seen.headers
    );
    assert_eq!(
        header(&seen.headers, "content-type"),
        Some("application/json"),
        "{:?}",
        seen.headers
    );
    let replayed: Value = serde_json::from_slice(&seen.body).expect("the body is the JSON sent");
    assert_eq!(&replayed, body, "the body, verbatim");
}

#[tokio::test]
async fn what_a_replica_cannot_serve_is_replayed_to_the_factory() {
    let factory = start_factory().await;
    let t = throwaway_org(&seeded_tenant().await).await;

    // Three workspaces, the same app published into each. Every working copy
    // is dropped once compiled, as it is absent from a replica.
    let servable_root = write_config(&postgres_entry("pg").await);
    write_view(servable_root.path());
    let servable = compile(&t, servable_root.path()).await;
    publish_app(&t, "servable", servable, &functions()).await;
    drop(servable_root);

    // A DuckDB file in the checkout. No `OXY_COMPILE_BLOB_S3_BUCKET` in a test
    // process, so the compiler mirrors nothing: the data is only on the node
    // that compiled it.
    let duck_root = write_config("  - name: duck\n    type: duckdb\n    path: local.duckdb\n");
    let needs_working_copy = compile(&t, duck_root.path()).await;
    let duck_app = publish_app(&t, "working-copy-db", needs_working_copy, &functions()).await;
    drop(duck_root);

    let not_compiled = uncompiled_workspace(&t).await;
    let bare_app = publish_app(&t, "uncompiled", not_compiled, &functions()).await;

    let _replica = AsDisklessReplica::enter();

    // Servable: answered here, and the Factory is not asked.
    let local = call_function_in(&t.org_slug, "servable", READS, json!({})).await;
    assert_eq!(local.status, StatusCode::OK, "stream: {}", local.raw);
    assert_eq!(local.frame("done"), Some(&json!({ "status": 200 })));
    let data = local
        .frame("data")
        .unwrap_or_else(|| panic!("no data frame; stream: {}", local.raw));
    assert!(
        data["semantic"].to_string().contains("from-compiled"),
        "answered from the compiled model, here: {data}"
    );
    assert!(
        !local.headers.contains_key("x-oxy-forwarded-via"),
        "a local answer is not a relayed one: {:?}",
        local.headers
    );
    assert_ne!(
        header(&local.headers, "x-oxy-served-by"),
        Some(FACTORY_SERVED_BY),
        "a local answer does not carry the Factory's marker"
    );
    assert!(
        arrived_at(&factory).await.is_empty(),
        "a reachable Factory must not pull a call a replica can run itself"
    );

    // A database that is a file in the checkout: replayed, headers and all.
    let relayed = call_function_with(
        &t.org_slug,
        "working-copy-db",
        READS,
        json!({ "n": 1 }),
        &[("idempotency-key", "k-duck")],
    )
    .await;
    assert_factory_answered(&relayed);
    let seen = arrived_at(&factory).await;
    assert_eq!(seen.len(), 1, "exactly one request crossed the hop");
    assert_replayed(
        &seen[0],
        &format!("/customer-apps/{}/working-copy-db/fn/{READS}", t.org_slug),
        &json!({ "n": 1 }),
    );
    assert_eq!(
        header(&seen[0].headers, "idempotency-key"),
        Some("k-duck"),
        "the caller's headers cross with the call — the Factory takes the \
         idempotency row, so it must see the key"
    );
    assert_eq!(
        queued_compiles(&t, needs_working_copy).await,
        0,
        "the workspace is compiled; a compile per call would change nothing"
    );
    assert!(
        invocations(&t.db, duck_app.app_id, READS).await.is_empty(),
        "no invocation row here: a `running` row would make the Factory refuse \
         the replayed call as a concurrent duplicate"
    );

    // Nothing compiled: replayed the same way, and a compile is queued.
    let relayed = call_function_in(&t.org_slug, "uncompiled", READS, json!({ "n": 2 })).await;
    assert_factory_answered(&relayed);
    let seen = arrived_at(&factory).await;
    assert_eq!(seen.len(), 2, "the second call crossed the hop too");
    assert_replayed(
        &seen[1],
        &format!("/customer-apps/{}/uncompiled/fn/{READS}", t.org_slug),
        &json!({ "n": 2 }),
    );
    assert_eq!(
        queued_compiles(&t, not_compiled).await,
        1,
        "the hop queues the compile that makes the next call servable here"
    );
    assert!(
        invocations(&t.db, bare_app.app_id, READS).await.is_empty(),
        "refused before the invocation row"
    );

    assert_eq!(
        leaks(),
        0,
        "deciding and forwarding must not reach for the disk"
    );
}

/// A refusal produced on this replica: the 503 and error name, `Retry-After`
/// as the caller's pacing, and no claim of a hop that did not happen.
fn assert_refused_here(call: &FnCall, error: &str) {
    assert_eq!(
        call.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a down Factory must not surface as the generic ide-down 502: {} {:?}",
        call.raw,
        call.headers
    );
    let body: Value = serde_json::from_str(&call.raw).expect("a JSON refusal");
    assert_eq!(body["error"], error, "{body}");
    assert!(
        !call.headers.contains_key("x-oxy-forwarded-via"),
        "nothing was relayed: {:?}",
        call.headers
    );
}

#[tokio::test]
async fn with_the_factory_down_a_replica_answers_the_documented_503s() {
    point_at_a_down_factory();
    let t = throwaway_org(&seeded_tenant().await).await;

    let duck_root = write_config("  - name: duck\n    type: duckdb\n    path: local.duckdb\n");
    let needs_working_copy = compile(&t, duck_root.path()).await;
    let duck_app = publish_app(&t, "working-copy-db", needs_working_copy, &functions()).await;
    drop(duck_root);

    let not_compiled = uncompiled_workspace(&t).await;
    let bare_app = publish_app(&t, "uncompiled", not_compiled, &functions()).await;

    let _replica = AsDisklessReplica::enter();

    // Nothing compiled: retryable, and the compile that ends the need for the
    // Factory is queued even though the Factory never heard of the call.
    let call = call_function_in(&t.org_slug, "uncompiled", READS, json!({})).await;
    assert_refused_here(&call, "WorkspaceNotCompiled");
    assert_eq!(
        header(&call.headers, "x-oxy-needs-recompile"),
        Some(not_compiled.to_string().as_str()),
        "the header a client retries on: {:?}",
        call.headers
    );
    assert!(
        call.headers.contains_key("retry-after"),
        "{:?}",
        call.headers
    );
    assert_eq!(
        queued_compiles(&t, not_compiled).await,
        1,
        "queued before the hop was tried"
    );
    assert!(invocations(&t.db, bare_app.app_id, READS).await.is_empty());

    // A database that is a file in the checkout: names the pod it needs, and
    // carries no recompile header — no compile makes the file appear here.
    let call = call_function_in(&t.org_slug, "working-copy-db", READS, json!({})).await;
    assert_refused_here(&call, "WorkspaceNeedsWorkingCopy");
    assert_eq!(header(&call.headers, "x-oxy-required-role"), Some("ide"));
    assert!(!call.headers.contains_key("x-oxy-needs-recompile"));
    assert_eq!(queued_compiles(&t, needs_working_copy).await, 0);
    assert!(invocations(&t.db, duck_app.app_id, READS).await.is_empty());

    assert_eq!(leaks(), 0, "trying the Factory must not reach for the disk");
}
