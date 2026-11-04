//! Shared fixtures for the runtime's tests — `MockHost`, `test_ctx`, the
//! `run_*` helpers — and the topic modules that use them.

mod attachments_and_notify;
mod broker_gates;
mod crypto;
mod host_call_failures;
mod identity;
mod limits_and_meters;
mod op_units;
mod request;
mod span_targets;
mod stores;

use super::ops::reply_json;
use super::*;
use crate::custom_apps_functions::host_error::HostError;

// What `runtime.rs` had in scope before it was split, and the topic modules
// still reach through `use super::*`. Kept as one block so the split moved
// names, not meanings.
use super::super::env_policy::{EnvPolicy, HeldTarget, HostOp};
use super::super::host::FunctionCapabilities;
use super::host_call::HostCall;
use std::sync::atomic::{AtomicBool, Ordering};

/// Test host: `ctx.query` returns one row and `ctx.email.send` records what
/// actually arrived, so a test can assert a payload survived the isolate
/// boundary byte-for-byte. Everything else is unused.
#[derive(Default)]
pub(super) struct MockHost {
    last_email: std::sync::Mutex<Option<serde_json::Value>>,
    /// What the last `ctx.notify` handed the host.
    last_notify: std::sync::Mutex<Option<serde_json::Value>>,
    /// Every `ctx.tx` op this host saw, in order — the bootstrap wrapper's
    /// commit/rollback bracket is only observable from here.
    tx_ops: std::sync::Mutex<Vec<String>>,
    /// When set, `ctx.query` fails with this message instead of answering.
    query_error: Option<String>,
    /// With `query_error`: only the first `ctx.query` fails; later ones
    /// answer, the way a flaky store does for a handler that retries.
    query_error_once: bool,
    /// How many `ctx.query` calls have reached the host.
    query_calls: std::sync::atomic::AtomicUsize,
    /// When set, `ctx.query` takes this long to answer.
    query_delay: Option<std::time::Duration>,
    /// When set, the app's own stores fail with this message: every
    /// `ctx.airhouse` op, and the `begin_oltp` that opens `ctx.oltp.tx`.
    own_store_error: Option<String>,
    /// Every `(op, kind, message, target)` the broker noted, in order —
    /// all of them, not just the first, so a test can see a note that
    /// should not exist.
    #[allow(clippy::type_complexity)]
    host_call_failures: std::sync::Mutex<Vec<(&'static str, &'static str, String, Option<String>)>>,
    /// Every `(op, target)` the broker reported a success for, in order.
    host_call_successes: std::sync::Mutex<Vec<(&'static str, Option<String>)>>,
    /// When set, `ctx.platform.facts` fails with this message instead of
    /// echoing its options back.
    platform_facts_error: Option<String>,
    /// What the manifest grants; `None` grants everything, so the ops
    /// above are reached. A test of the broker's gate sets a narrower one.
    caps: Option<FunctionCapabilities>,
    /// The environment policy; `None` is production.
    policy: Option<EnvPolicy>,
    /// Every `(op, "plane namespace verb table")` the broker held before
    /// dispatch, in order.
    held: std::sync::Mutex<Vec<(HostOp, String)>>,
}

impl MockHost {
    fn tx_ops(&self) -> Vec<String> {
        self.tx_ops.lock().unwrap().clone()
    }
    fn host_call_failures(&self) -> Vec<(&'static str, &'static str)> {
        self.host_call_failures
            .lock()
            .unwrap()
            .iter()
            .map(|(op, kind, _, _)| (*op, *kind))
            .collect()
    }
    /// The message each note carried, in order: the real host folds it
    /// into the fingerprint (`HostCallFailure::noted`).
    fn host_call_failure_messages(&self) -> Vec<String> {
        self.host_call_failures
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, message, _)| message.clone())
            .collect()
    }
    /// The target each note carried, in order: what the real host keys a
    /// later success against (`HostCallFailure::recovered`).
    fn host_call_failure_targets(&self) -> Vec<Option<String>> {
        self.host_call_failures
            .lock()
            .unwrap()
            .iter()
            .map(|(_, _, _, target)| target.clone())
            .collect()
    }
    fn host_call_successes(&self) -> Vec<&'static str> {
        self.host_call_successes
            .lock()
            .unwrap()
            .iter()
            .map(|(op, _)| *op)
            .collect()
    }
    fn host_call_successes_with_targets(&self) -> Vec<(&'static str, Option<String>)> {
        self.host_call_successes.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl FunctionHost for MockHost {
    fn capabilities(&self) -> &FunctionCapabilities {
        static EVERYTHING: std::sync::LazyLock<FunctionCapabilities> =
            std::sync::LazyLock::new(FunctionCapabilities::granting_everything);
        self.caps.as_ref().unwrap_or(&EVERYTHING)
    }
    fn env_policy(&self) -> &EnvPolicy {
        static PRODUCTION: std::sync::LazyLock<EnvPolicy> =
            std::sync::LazyLock::new(EnvPolicy::production);
        self.policy.as_ref().unwrap_or(&PRODUCTION)
    }
    fn decides_by_environment(&self) -> bool {
        self.policy.as_ref().is_some_and(|p| !p.is_production())
    }
    async fn record_held(&self, op: HostOp, (plane, namespace, verb, table): HeldTarget<'_>) {
        self.held
            .lock()
            .unwrap()
            .push((op, format!("{plane} {namespace} {verb} {table}")));
    }
    async fn query(&self, _sql: String) -> Result<serde_json::Value, HostError> {
        let calls_before = self
            .query_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(delay) = self.query_delay {
            tokio::time::sleep(delay).await;
        }
        if let Some(message) = &self.query_error
            && !(self.query_error_once && calls_before > 0)
        {
            return Err(message.clone().into());
        }
        Ok(serde_json::json!({ "rows": [{ "x": 1 }], "truncated": false }))
    }
    fn note_host_call_failure(
        &self,
        op: &'static str,
        kind: &'static str,
        message: &str,
        target: Option<&str>,
    ) {
        self.host_call_failures.lock().unwrap().push((
            op,
            kind,
            message.to_string(),
            target.map(str::to_string),
        ));
    }
    fn note_host_call_success(&self, op: &'static str, target: Option<&str>) {
        self.host_call_successes
            .lock()
            .unwrap()
            .push((op, target.map(str::to_string)));
    }
    async fn tx(
        &self,
        op: String,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        self.tx_ops.lock().unwrap().push(op.clone());
        if let (Some(message), "begin_oltp") = (&self.own_store_error, op.as_str()) {
            return Err(message.clone().into());
        }
        match op.as_str() {
            "begin" | "begin_oltp" => Ok(serde_json::json!({ "id": 1 })),
            // Echo the params back so a test can assert they crossed the
            // boundary as an array rather than being stringified.
            "query" => Ok(serde_json::json!({
                "rows": [{ "id": 42, "params": payload.get("params").cloned() }]
            })),
            "exec" => Ok(serde_json::json!({ "rowCount": 1 })),
            "commit" | "rollback" => Ok(serde_json::json!({ "ok": true })),
            other => Err(format!("unexpected tx op '{other}'").into()),
        }
    }
    async fn platform_facts(
        &self,
        opts: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        match &self.platform_facts_error {
            Some(message) => Err(message.clone().into()),
            // Echo what crossed the boundary, so a test sees the options
            // arrive as an object rather than a stringified blob.
            None => Ok(serde_json::json!({ "echo": opts })),
        }
    }
    async fn send_email(&self, input: serde_json::Value) -> Result<serde_json::Value, HostError> {
        *self.last_email.lock().unwrap() = Some(input);
        Ok(serde_json::json!({ "messageId": "test-message-id" }))
    }
    async fn notify(&self, input: serde_json::Value) -> Result<serde_json::Value, HostError> {
        *self.last_notify.lock().unwrap() = Some(input);
        Ok(serde_json::json!({ "notificationId": "n-1", "recipients": 2, "skipped": [] }))
    }
    async fn query_stream(&self, _sql: String) -> Result<serde_json::Value, HostError> {
        Err("unused".into())
    }
    async fn fetch(
        &self,
        _url: String,
        _init: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        Err("unused".into())
    }
    async fn semantic_query(
        &self,
        _spec: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        Err("unused".into())
    }
    async fn airway_run(
        &self,
        _pipeline_ref: String,
        _variables: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        Err("unused".into())
    }
    async fn warehouse_query(
        &self,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        // Echo the database back so a test can prove the name crossed the
        // isolate boundary rather than being defaulted host-side.
        Ok(serde_json::json!({
            "rows": [{ "x": 1 }],
            "truncated": false,
            "db": payload["database"],
        }))
    }
    async fn warehouse_write(
        &self,
        _op: String,
        _payload: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        Err("unused".into())
    }
    async fn oltp(
        &self,
        op: String,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        // Echo sql/params so a test can prove the call crossed the isolate
        // boundary intact — params as an array, not a stringified blob.
        match op.as_str() {
            "query" => Ok(serde_json::json!({
                "rows": [{ "sql": payload.get("sql"), "params": payload.get("params") }]
            })),
            "exec" => Ok(serde_json::json!({ "rowCount": 1 })),
            other => Err(format!("unexpected oltp op '{other}'").into()),
        }
    }
    async fn airhouse(
        &self,
        op: String,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        if let Some(message) = &self.own_store_error {
            return Err(message.clone().into());
        }
        // Echo what crossed the boundary: the row count proves `rows`
        // arrived as an array, not a stringified blob.
        match op.as_str() {
            "append" => Ok(serde_json::json!({
                "rowCount": payload["rows"].as_array().map_or(0, |r| r.len()),
            })),
            "query" => Ok(serde_json::json!({
                "rows": [{ "sql": payload["sql"] }],
                "truncated": false,
            })),
            "exec" => Ok(serde_json::json!({ "ok": true })),
            other => Err(format!("unexpected airhouse op '{other}'").into()),
        }
    }
    async fn secrets_set(
        &self,
        _key: String,
        _value: String,
    ) -> Result<serde_json::Value, HostError> {
        Err("unused".into())
    }
    async fn storage(
        &self,
        _op: String,
        _payload: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        Err("unused".into())
    }
}

pub(super) fn test_ctx() -> InvocationCtx {
    InvocationCtx {
        user: CtxUser {
            id: "u".into(),
            email: Some("e@example.com".into()),
            org_id: "o".into(),
            name: None,
            picture: None,
            app_role: None,
            org_role: None,
            teams: Vec::new(),
            kind: CtxIdentityKind::User,
            reach: oxy_app_core::operating_graph::reach::Reach::nowhere(),
        },
        env: Default::default(),
        airhouse_schema: None,
        channel: "production".into(),
    }
}

/// Run `artifact` against a fresh `MockHost` and hand back both, so a test
/// can assert on the response *and* on what reached the host.
async fn run_with_mock(artifact: &str) -> (Result<FnResponse, RuntimeError>, Arc<MockHost>) {
    run_with_mock_ctx(artifact, test_ctx()).await
}

/// [`run_with_mock`] with a caller-supplied identity, for asserting what the
/// isolate actually sees on `ctx.user`.
async fn run_with_mock_ctx(
    artifact: &str,
    ctx: InvocationCtx,
) -> (Result<FnResponse, RuntimeError>, Arc<MockHost>) {
    let host = Arc::new(MockHost::default());
    (run_on(artifact, ctx, host.clone()).await, host)
}

/// Run `artifact` against a caller-built `host`, for a test that needs the
/// host to fail in a particular way.
async fn run_on(
    artifact: &str,
    ctx: InvocationCtx,
    host: Arc<MockHost>,
) -> Result<FnResponse, RuntimeError> {
    run_request(artifact, ctx, host, FnRequest::from_body(b"{}".to_vec())).await
}

/// [`run_on`] with a caller-supplied request, for a test about how the
/// request reaches the handler or what it costs to deliver.
async fn run_request(
    artifact: &str,
    ctx: InvocationCtx,
    host: Arc<MockHost>,
    req: FnRequest,
) -> Result<FnResponse, RuntimeError> {
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    run(
        artifact.to_string(),
        // A fresh org per helper call, so concurrent tests never contend on
        // one org's admission semaphore and time each other out.
        uuid::Uuid::new_v4(),
        ctx,
        req,
        host,
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
}

/// A handler that makes one call and answers what it threw, or what it got.
fn attempt(call: &str) -> String {
    format!(
        "export default async (req, ctx) => {{
            try {{ return Response.json({{ ok: await {call} }}); }}
            catch (e) {{ return Response.json({{ error: String(e.message || e) }}); }}
        }};"
    )
}

/// Each call's trace record, from a real run against this file's host.
#[path = "call_trace_tests.rs"]
mod call_trace_tests;
