/// Every host-op span must carry [`HOST_CALL_TARGET`]: the product
/// collector keeps them out of the tenant Traces console with an
/// `oxy::host_call=off` directive, and a span that falls back to the
/// module target slips past it. A new `HostCall` variant fails the
/// exhaustive `match` below until it is added to the list.
#[test]
fn every_host_call_span_carries_the_platform_only_target() {
    use super::super::host_call_attrs::HOST_CALL_TARGET;
    use tokio::sync::oneshot;
    fn reply() -> oneshot::Sender<Result<serde_json::Value, HostError>> {
        oneshot::channel().0
    }
    let json = serde_json::Value::Null;
    let calls = vec![
        HostCall::Query {
            sql: "select 1".into(),
            reply: reply(),
        },
        HostCall::QueryStream {
            sql: "select 1".into(),
            reply: reply(),
        },
        HostCall::Fetch {
            url: "https://example.test/".into(),
            init: json.clone(),
            reply: reply(),
        },
        HostCall::SemanticQuery {
            spec: json.clone(),
            reply: reply(),
        },
        HostCall::AirwayRun {
            pipeline_ref: "p".into(),
            variables: json.clone(),
            reply: reply(),
        },
        HostCall::WarehouseWrite {
            op: "query".into(),
            payload: json.clone(),
            reply: reply(),
        },
        HostCall::SecretsSet {
            key: "k".into(),
            value: "v".into(),
            reply: reply(),
        },
        HostCall::SendEmail {
            input: json.clone(),
            reply: reply(),
        },
        HostCall::Notify {
            input: json.clone(),
            reply: reply(),
        },
        HostCall::Storage {
            op: "put".into(),
            payload: json.clone(),
            reply: reply(),
        },
        HostCall::OrgPeople { reply: reply() },
        HostCall::OrgPlaces { reply: reply() },
        HostCall::OrgAssignments { reply: reply() },
        HostCall::PlatformFacts {
            opts: json.clone(),
            reply: reply(),
        },
        HostCall::Tx {
            op: "begin".into(),
            payload: json.clone(),
            reply: reply(),
        },
        HostCall::Oltp {
            op: "query".into(),
            payload: json.clone(),
            reply: reply(),
        },
        HostCall::Airhouse {
            op: "append".into(),
            payload: json,
            reply: reply(),
        },
    ];
    for call in &calls {
        // Exhaustive on purpose: a new variant must be listed above.
        match call {
            HostCall::Query { .. }
            | HostCall::QueryStream { .. }
            | HostCall::Fetch { .. }
            | HostCall::SemanticQuery { .. }
            | HostCall::AirwayRun { .. }
            | HostCall::WarehouseWrite { .. }
            | HostCall::SecretsSet { .. }
            | HostCall::SendEmail { .. }
            | HostCall::Notify { .. }
            | HostCall::Storage { .. }
            | HostCall::OrgPeople { .. }
            | HostCall::OrgPlaces { .. }
            | HostCall::OrgAssignments { .. }
            | HostCall::PlatformFacts { .. }
            | HostCall::Tx { .. }
            | HostCall::Oltp { .. }
            | HostCall::Airhouse { .. } => {}
        }
    }
    // A bare registry enables every span, so `metadata()` is `Some`.
    tracing::subscriber::with_default(tracing_subscriber::registry(), || {
        for call in &calls {
            let (span, _, _) = host_call_span(call);
            let meta = span.metadata().expect("span is enabled under a registry");
            assert_eq!(
                meta.target(),
                HOST_CALL_TARGET,
                "span `{}` would leak into the product store",
                meta.name()
            );
        }
    });
}

/// The cap has to actually cap. The first version returned at the marker
/// without pushing it, so the buffer parked at exactly MAX forever: every
/// later call saw `Equal`, re-emitted the marker, and `Silent` was
/// unreachable — 99,500 identical warn events for a 100k-line loop, each
/// retained in the span until close. Pushing the marker is what advances
/// the count past MAX and makes the third state reachable.
#[test]
fn the_log_cap_emits_exactly_one_marker_then_goes_silent() {
    use super::ops::{LogAction, MAX_CAPTURED_LOGS, log_action};

    assert_eq!(log_action(0), LogAction::Emit);
    assert_eq!(log_action(MAX_CAPTURED_LOGS - 1), LogAction::Emit);
    assert_eq!(log_action(MAX_CAPTURED_LOGS), LogAction::Marker);
    // The marker pushes a line, so the very next call is already over.
    assert_eq!(log_action(MAX_CAPTURED_LOGS + 1), LogAction::Silent);

    // Simulate the loop the cap exists for: 10k lines, one marker.
    let mut buffered = 0usize;
    let mut markers = 0;
    let mut emitted = 0;
    for _ in 0..10_000 {
        match log_action(buffered) {
            LogAction::Emit => {
                emitted += 1;
                buffered += 1;
            }
            LogAction::Marker => {
                markers += 1;
                buffered += 1;
            }
            LogAction::Silent => {}
        }
    }
    assert_eq!(
        markers, 1,
        "the marker must fire once, not per dropped line"
    );
    assert_eq!(emitted, MAX_CAPTURED_LOGS);
    assert_eq!(buffered, MAX_CAPTURED_LOGS + 1);
}
use super::ops::reply_json;
use super::*;
use crate::custom_apps_functions::host_error::HostError;

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

#[test]
fn reply_json_ok_passes_value_through() {
    let out = reply_json("ctx.query", Ok(serde_json::json!({ "rows": [] })));
    assert_eq!(out, r#"{"rows":[]}"#);
}

#[test]
fn reply_json_err_wraps_as_oxy_error() {
    let out = reply_json("ctx.warehouse", Err("boom".into()));
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["__oxyError"], "HostError");
    assert_eq!(parsed["message"], "ctx.warehouse: boom");
}

#[test]
fn cancelled_flag_defaults_false() {
    let flag = Arc::new(AtomicBool::new(false));
    assert!(!flag.load(Ordering::Relaxed));
    flag.store(true, Ordering::Relaxed);
    assert!(flag.load(Ordering::Relaxed));
}

fn full_identity() -> CtxUser {
    CtxUser {
        id: "11111111-1111-1111-1111-111111111111".into(),
        email: Some("ada@acme.com".into()),
        org_id: "22222222-2222-2222-2222-222222222222".into(),
        name: Some("Ada Lovelace".into()),
        picture: Some("https://cdn.example/ada.png".into()),
        app_role: Some("admin".into()),
        org_role: Some("member".into()),
        teams: vec![CtxTeam {
            id: "33333333-3333-3333-3333-333333333333".into(),
            name: "Finance".into(),
        }],
        kind: CtxIdentityKind::User,
        reach: oxy_app_core::operating_graph::reach::Reach::everywhere("app-admin", Vec::new()),
    }
}

/// The wire contract the SDK's `OxyFunctionUser` is typed against. Every key
/// here is camelCase — `org_id` shipped snake-cased once and made the
/// documented `ctx.user.orgId` read `undefined`, so the casing is pinned.
#[test]
fn ctx_user_serializes_the_documented_camel_case_keys() {
    let json: serde_json::Value = serde_json::to_value(full_identity()).unwrap();
    assert_eq!(json["orgId"], "22222222-2222-2222-2222-222222222222");
    assert_eq!(json["appRole"], "admin");
    assert_eq!(json["orgRole"], "member");
    assert_eq!(json["name"], "Ada Lovelace");
    assert_eq!(json["picture"], "https://cdn.example/ada.png");
    assert_eq!(json["kind"], "user");
    assert_eq!(json["teams"][0]["name"], "Finance");
    assert!(
        json.get("org_id").is_none(),
        "the host serializes orgId; the snake alias is added in __buildCtx, not here"
    );
}

#[test]
fn a_frontline_user_serializes_email_as_null_not_absent_and_not_empty() {
    // The contract app code branches on. `null` is load-bearing three ways:
    // `""` would look like an address to `ctx.email.send` and to any string
    // concat; an ABSENT key reads as `undefined`, which a `=== null` check
    // misses; and only an explicit null lets a function say "this person
    // cannot be emailed" without guessing.
    let json: serde_json::Value = serde_json::to_value(CtxUser {
        id: "11111111-1111-1111-1111-111111111111".into(),
        email: None,
        org_id: "22222222-2222-2222-2222-222222222222".into(),
        name: Some("Maria S.".into()),
        picture: None,
        app_role: None,
        org_role: None,
        teams: Vec::new(),
        kind: CtxIdentityKind::User,
        reach: oxy_app_core::operating_graph::reach::Reach::nowhere(),
    })
    .unwrap();
    assert_eq!(
        json.get("email"),
        Some(&serde_json::Value::Null),
        "email must be present and null, never omitted or empty: {json}"
    );
}

/// A schedule tick runs under the org owner's `user_id` but has no human
/// behind it. Absent human fields are what lets a function tell the two
/// apart without sniffing the synthetic email.
#[test]
fn system_identity_omits_every_human_field() {
    let json: serde_json::Value = serde_json::to_value(CtxUser {
        email: Some("schedule+rollup@system.oxy".into()),
        name: None,
        picture: None,
        org_role: None,
        teams: Vec::new(),
        kind: CtxIdentityKind::System,
        ..full_identity()
    })
    .unwrap();
    assert_eq!(json["kind"], "system");
    for absent in ["name", "picture", "orgRole"] {
        assert!(json.get(absent).is_none(), "{absent} must be absent");
    }
    assert_eq!(json["teams"], serde_json::json!([]));
}

/// The isolate's view, end to end: `ctx.user.orgId` is what the SDK types
/// promise, and the legacy `ctx.user.org_id` still resolves so functions
/// written against the shipped snake key keep working.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn isolate_sees_camel_case_identity_and_the_legacy_org_id_alias() {
    let (result, _host) = run_with_mock_ctx(
        r#"
            export default async (req, ctx) => Response.json({
                orgId: ctx.user.orgId,
                legacy: ctx.user.org_id,
                name: ctx.user.name,
                orgRole: ctx.user.orgRole,
                team: ctx.user.teams[0].name,
                kind: ctx.user.kind,
            });
        "#,
        InvocationCtx {
            user: full_identity(),
            env: Default::default(),
            airhouse_schema: None,
            channel: "production".into(),
        },
    )
    .await;

    let body = result.expect("function must resolve").body;
    let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(parsed["orgId"], "22222222-2222-2222-2222-222222222222");
    assert_eq!(
        parsed["legacy"], parsed["orgId"],
        "the back-compat mirror must track orgId, not drift from it"
    );
    assert_eq!(parsed["name"], "Ada Lovelace");
    assert_eq!(parsed["orgRole"], "member");
    assert_eq!(parsed["team"], "Finance");
    assert_eq!(parsed["kind"], "user");
}

// ── The broker's gates: capability, environment, registry ───────────────

/// A handler that makes one call and answers what it threw, or what it got.
fn attempt(call: &str) -> String {
    format!(
        "export default async (req, ctx) => {{
            try {{ return Response.json({{ ok: await {call} }}); }}
            catch (e) {{ return Response.json({{ error: String(e.message || e) }}); }}
        }};"
    )
}

/// A manifest that grants nothing refuses a gated op with the host's own
/// words before the call is dispatched: the mock's `send_email` records
/// nothing, and the refusal is noted as a failure the way any other
/// `not_allowed` is.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_broker_refuses_an_ungranted_capability_before_the_host() {
    let host = Arc::new(MockHost {
        caps: Some(FunctionCapabilities::default()),
        ..Default::default()
    });
    let resp = run_on(
        &attempt(r#"ctx.email.send({ to: "a@b.com", subject: "s", text: "t" })"#),
        test_ctx(),
        host.clone(),
    )
    .await
    .unwrap();
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.starts_with("ctx.email.send: EmailCapabilityMissing: this function has not declared"),
        "{error}"
    );
    assert!(
        host.last_email.lock().unwrap().is_none(),
        "never dispatched"
    );
    assert_eq!(host.host_call_failures(), [("email.send", "not_allowed")]);
}

/// Outside production the broker decides every op that holds by not running:
/// a refused one is answered in the policy's words and noted in the held
/// log under its plane and the identifiers it named; an isolated one goes
/// on to the host, which routes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_broker_holds_and_refuses_by_the_environment_before_the_host() {
    use oxy_app_core::custom_app_environment::AppEnvironment;
    let host = Arc::new(MockHost {
        policy: Some(EnvPolicy::for_environment(AppEnvironment::Staging)),
        ..Default::default()
    });
    let resp = run_on(
        &attempt(r#"ctx.airway.run("orders_sync")"#),
        test_ctx(),
        host.clone(),
    )
    .await
    .unwrap();
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("EnvironmentRefused: ctx.airway.run is refused in the staging environment"),
        "{error}"
    );
    assert_eq!(
        host.held.lock().unwrap().clone(),
        [(HostOp::AirwayRun, "airway orders_sync RUN ".to_string())]
    );
    // Refused by the policy, not failed: nothing to page.
    assert!(host.host_call_failures().is_empty());

    // An isolated op is the host's to route: `secrets.set` reaches the mock,
    // whose answer is its own.
    let resp = run_on(
        &attempt(r#"ctx.secrets.set("K", "v")"#),
        test_ctx(),
        host.clone(),
    )
    .await
    .unwrap();
    let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(body["error"], "ctx.secrets.set: unused");
    assert_eq!(host.held.lock().unwrap().len(), 1, "nothing more held");
}

/// A sub-op with no registry row never reaches the host: the mock's
/// `storage` answers `unused`, and this is not that.
#[tokio::test]
async fn an_op_missing_from_the_registry_is_refused_by_the_broker() {
    let host: Arc<dyn FunctionHost> = Arc::new(MockHost::default());
    let (reply, _rx) = tokio::sync::oneshot::channel();
    let call = HostCall::Storage {
        op: "rename".into(),
        payload: serde_json::json!({ "key": "a" }),
        reply,
    };
    let (_reply, result) = dispatch_host_call(call, host).await;
    assert_eq!(result, Err("ctx.storage: unknown op 'rename'".into()));
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

/// The whole premise of the heap ceiling, asserted end to end.
///
/// Before it, `create_params` was unset and V8's own default limit sat far
/// above the pod's cgroup limit, so a runaway allocation was reaped by the
/// kernel OOM killer — which kills the **process**, taking down every other
/// app and every product route it was serving. This test allocates without
/// bound and asserts the isolate dies alone.
///
/// The test process surviving to make the assertion *is* half the
/// assertion: without a working ceiling this test does not fail, it takes
/// the test binary with it.
#[tokio::test]
async fn a_runaway_allocation_kills_the_isolate_not_the_process() {
    // Set before the first `heap_limit_bytes()` call in this process —
    // nextest gives each test its own process, so the OnceLock is ours.
    // 16 MiB rather than the 128 MiB default so it trips in well under the
    // 10s helper timeout; the mechanism under test is identical.
    unsafe { std::env::set_var(HEAP_LIMIT_MB_ENV, "16") };
    assert_eq!(heap_limit_bytes(), Some(16 * 1024 * 1024));

    let (result, _host) = run_with_mock_ctx(
        r#"
            export default async function () {
              const held = [];
              // Retained, so GC cannot reclaim any of it — an unretained loop
              // would spin forever instead of breaching the ceiling.
              for (;;) { held.push(new Array(1_000_000).fill(7)); }
            }
            "#,
        test_ctx(),
    )
    .await;

    assert!(
        matches!(result, Err(RuntimeError::OutOfMemory)),
        "a heap breach must be attributable, not a generic failure: {result:?}"
    );
}

/// The ceiling must be switchable off, and `0` is the off switch. An
/// operator debugging a memory-hungry function needs a way to lift it
/// without a rebuild, and "unset" already means the default.
#[test]
fn a_zero_heap_limit_disables_the_ceiling() {
    unsafe { std::env::set_var(HEAP_LIMIT_MB_ENV, "0") };
    assert_eq!(heap_limit_bytes(), None);
}

/// Delivering the request body must cost the isolate's heap one copy of it.
///
/// The body used to be spliced into the invoke script as a string literal,
/// and V8 keeps a script's source for as long as the script runs. So the
/// body was on the heap twice before the handler's first line — once as
/// source, once as the literal's value — and both counted against the
/// ceiling the handler then had to work under.
///
/// Measured that way, under a 64 MiB ceiling: with no body a handler held
/// 40 MiB, and beside a 28 MiB body it was terminated holding 2. Here it
/// holds 12 MiB beside the same body, in pieces small enough that V8 has to
/// find room for each — the ceiling is enforced by collections that free
/// nothing, so one large allocation that crosses it and returns would not
/// show the difference.
#[tokio::test]
async fn delivering_the_request_body_costs_the_heap_one_copy_of_it() {
    // Set before the first `heap_limit_bytes()` call in this process, as in
    // `a_runaway_allocation_kills_the_isolate_not_the_process`.
    unsafe { std::env::set_var(HEAP_LIMIT_MB_ENV, "64") };
    assert_eq!(heap_limit_bytes(), Some(64 * 1024 * 1024));

    const BODY_BYTES: usize = 28 * 1024 * 1024;
    let result = run_request(
        r#"
            export default async (req) => {
              const held = [];
              // 64 KiB a piece, 12 MiB in all.
              for (let i = 0; i < 12 * 16; i++) held.push(new Array(8192).fill(i));
              return Response.json({ chars: req.body.length, pieces: held.length });
            };
            "#,
        test_ctx(),
        Arc::new(MockHost::default()),
        FnRequest::from_body(vec![b'A'; BODY_BYTES]),
    )
    .await;

    let resp = result.expect("12 MiB of work fits beside one copy of a 28 MiB body");
    let parsed: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(parsed["chars"], BODY_BYTES);
    assert_eq!(parsed["pieces"], 12 * 16);
}

/// However the body is handed over, `req` must read exactly as it did when
/// it was a JSON literal: the same three keys in the same order, and a body
/// that is the request's bytes as text — including the characters a string
/// literal has to escape, and U+FFFD wherever the bytes were not UTF-8.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_reads_the_same_however_the_body_is_handed_over() {
    let mut body = "quote \" backslash \\ newline \n nul \0 tag </script> \
                        separators \u{2028}\u{2029} astral \u{1F600} ${not} `a template`"
        .as_bytes()
        .to_vec();
    // A lone continuation byte: not UTF-8 on its own.
    body.extend_from_slice(b" invalid \x80 end");
    let expected = String::from_utf8_lossy(&body).into_owned();
    assert!(expected.contains('\u{FFFD}'));

    let resp = run_request(
        r#"
            export default async (req) =>
              Response.json({ keys: Object.keys(req), body: req.body, type: typeof req.body });
            "#,
        test_ctx(),
        Arc::new(MockHost::default()),
        FnRequest::from_body(body),
    )
    .await
    .expect("handler must complete");

    let parsed: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(
        parsed["keys"],
        serde_json::json!(["method", "headers", "body"])
    );
    assert_eq!(parsed["type"], "string");
    assert_eq!(parsed["body"], expected);
}

/// `ctx.platform.facts` is bound, carries its options to the host as an
/// object (`{}` when called bare), and resolves to the host's reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_platform_facts_hands_the_host_its_options() {
    let (result, _host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
              const asked = await ctx.platform.facts({ weeks: 4 });
              const bare = await ctx.platform.facts();
              return Response.json({ asked, bare });
            };
            "#,
    )
    .await;
    let resp = result.expect("handler must complete");
    let parsed: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(parsed["asked"]["echo"], serde_json::json!({ "weeks": 4 }));
    assert_eq!(parsed["bare"]["echo"], serde_json::json!({}));
}

/// The internal app tells a refusal apart by the label inside the thrown
/// error's message, so the message has to arrive as
/// `ctx.platform.facts: <Label>: …`, label intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_platform_facts_call_throws_the_label_in_its_message() {
    let host = Arc::new(MockHost {
        platform_facts_error: Some(
            "PlatformFactsNotEnabled: this app's org is not an internal org".into(),
        ),
        ..Default::default()
    });
    let resp = run_on(
        r#"
            export default async (req, ctx) => {
              try { await ctx.platform.facts(); return new Response("resolved"); }
              catch (e) { return Response.json({ name: e.name, message: e.message }); }
            };
            "#,
        test_ctx(),
        host,
    )
    .await
    .expect("the handler caught the refusal and returned");
    let parsed: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(parsed["name"], "HostError");
    assert_eq!(
        parsed["message"],
        "ctx.platform.facts: PlatformFactsNotEnabled: this app's org is not an internal org"
    );
}

/// A handler that catches a failed `ctx.*` call and answers 200 is the
/// failure that never paged. The broker notes it on the host before the
/// isolate sees the rejection, so catching it cannot hide it.
const CATCHES_A_FAILED_QUERY: &str = r#"
        export default async (req, ctx) => {
            let seen = "none";
            try { await ctx.query("select 1"); } catch (e) { seen = e.message; }
            return new Response(seen);
        };
    "#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caught_host_call_failure_is_noted_on_the_host() {
    let host = Arc::new(MockHost {
        query_error: Some("connection refused".into()),
        ..Default::default()
    });
    let resp = run_on(CATCHES_A_FAILED_QUERY, test_ctx(), host.clone())
        .await
        .expect("the handler caught the failure and returned");
    assert_eq!(resp.status, 200);
    assert!(resp.body.contains("connection refused"), "{}", resp.body);
    assert_eq!(
        host.host_call_failures(),
        // `connection refused` is the connection never opening: the kind
        // that names it (`host_call_attrs::is_unreachable`), not the catch-all.
        vec![("query", "unreachable")],
        "exactly one note, for the one failed call"
    );
    assert_eq!(
        host.host_call_failure_messages(),
        vec!["connection refused"],
        "the note carries the host's message, for the fingerprint"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_not_found_host_call_failure_is_not_noted() {
    let host = Arc::new(MockHost {
        query_error: Some("object not found".into()),
        ..Default::default()
    });
    let resp = run_on(CATCHES_A_FAILED_QUERY, test_ctx(), host.clone())
        .await
        .expect("the handler caught the failure and returned");
    // The call did fail — so an empty note list means "not paged", not
    // "never reached the host".
    assert!(resp.body.contains("object not found"), "{}", resp.body);
    assert_eq!(host.host_call_failures(), Vec::new());
}

/// A handler that retries a failed call and gets its answer has recovered.
/// The broker reports the success under the same closed-list op name as
/// the failure, which is what lets the host's note clear
/// (`HostCallFailure::recovered`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retried_host_call_reports_its_success_under_the_same_op() {
    let host = Arc::new(MockHost {
        query_error: Some("connection refused".into()),
        query_error_once: true,
        ..Default::default()
    });
    let resp = run_on(
        r#"
            export default async (req, ctx) => {
                let rows;
                try { rows = await ctx.query("select 1"); }
                catch (e) { rows = await ctx.query("select 1"); }
                return Response.json({ rows });
            };
        "#,
        test_ctx(),
        host.clone(),
    )
    .await
    .expect("the retry answered");
    assert_eq!(resp.status, 200, "{}", resp.body);
    assert_eq!(host.host_call_failures(), vec![("query", "unreachable")]);
    assert_eq!(host.host_call_successes(), vec!["query"]);
    // `ctx.query` names no target, so the note is keyed by op alone.
    assert_eq!(host.host_call_failure_targets(), vec![None]);
    assert_eq!(
        host.host_call_successes_with_targets(),
        vec![("query", None)]
    );
}

/// The broker names where a call went alongside its op, on failure and on
/// success, so the host can clear a caught failure only for the same
/// target (`HostCallFailure::recovered`): a `fetch`'s host, by the one URL
/// parser; a `warehouse.*` op's database and table; nothing for an op
/// that names no place. Neither the URL's path nor the query reaches the
/// note.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_call_is_noted_with_its_target() {
    let host = Arc::new(MockHost::default());
    let resp = run_on(
            r#"
            export default async (req, ctx) => {
                try { await ctx.fetch("https://api.vendor-a.com/v1/rates?key=sk_live_abc"); } catch (e) {}
                try { await ctx.fetch("https://hooks.slack.com/services/T0/B0/x"); } catch (e) {}
                const r = await ctx.warehouse.query("dw", "SELECT 1");
                return Response.json({ rows: r.rows.length });
            };
        "#,
            test_ctx(),
            host.clone(),
        )
        .await
        .expect("the handler caught both fetches and answered");
    assert_eq!(resp.status, 200, "{}", resp.body);
    assert_eq!(
        host.host_call_failures(),
        vec![("fetch", "host_call_failed"), ("fetch", "host_call_failed")]
    );
    assert_eq!(
        host.host_call_failure_targets(),
        vec![
            Some("api.vendor-a.com".to_string()),
            Some("hooks.slack.com".to_string())
        ],
        "two hosts under one op are two targets"
    );
    assert_eq!(
        host.host_call_successes_with_targets(),
        vec![("warehouse.query", Some("dw".to_string()))]
    );
}

/// The note holds exactly what the call's span records, gated the same
/// way (`data_audit::record_db_span`): a database or table that is not
/// `identifier_like` — over 64 chars, or carrying anything outside
/// `[A-Za-z0-9_.$-]` — drops out of the target rather than into it, and a
/// fetch keeps the host alone.
#[test]
fn host_call_target_keeps_only_what_the_span_records() {
    fn warehouse(payload: serde_json::Value) -> HostCall {
        let (reply, _rx) = tokio::sync::oneshot::channel();
        HostCall::WarehouseWrite {
            op: "insert".into(),
            payload,
            reply,
        }
    }
    fn fetch(url: &str) -> HostCall {
        let (reply, _rx) = tokio::sync::oneshot::channel();
        HostCall::Fetch {
            url: url.into(),
            init: serde_json::json!({}),
            reply,
        }
    }
    let target = |call: HostCall| host_call_target(&call);
    assert_eq!(
        target(warehouse(
            serde_json::json!({ "database": "dw", "table": "ledger" })
        )),
        Some("dw ledger".into())
    );
    assert_eq!(
        target(warehouse(serde_json::json!({ "database": "dw" }))),
        Some("dw".into())
    );
    // A table the span would not record leaves the database alone.
    for table in ["x".repeat(65), "ledger; drop table x".into(), "".into()] {
        assert_eq!(
            target(warehouse(
                serde_json::json!({ "database": "dw", "table": table })
            )),
            Some("dw".into()),
            "{table}"
        );
    }
    // A database the span would not record is no target at all.
    assert_eq!(
        target(warehouse(
            serde_json::json!({ "database": "x".repeat(65), "table": "ledger" })
        )),
        None
    );
    assert_eq!(
        target(warehouse(serde_json::json!({ "table": "ledger" }))),
        None
    );
    // A fetch keeps the host: no userinfo, port, path or query.
    assert_eq!(
        target(fetch(
            "https://user:pw@api.example.com:8443/v1/x?key=sk_live_1"
        )),
        Some("api.example.com".into())
    );
    // An op that names no place.
    let (reply, _rx) = tokio::sync::oneshot::channel();
    assert_eq!(
        target(HostCall::Query {
            sql: "select 1".into(),
            reply
        }),
        None
    );
}

/// A refusal of the call's own arguments is the app's error, and the run
/// fails the same way every time; catching it is not hiding a failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caught_bad_request_is_not_noted() {
    let host = Arc::new(MockHost {
        query_error: Some("`sql` is required".into()),
        ..Default::default()
    });
    let resp = run_on(CATCHES_A_FAILED_QUERY, test_ctx(), host.clone())
        .await
        .expect("the handler caught the refusal and returned");
    assert!(resp.body.contains("`sql` is required"), "{}", resp.body);
    assert_eq!(host.host_call_failures(), Vec::new());
}

/// `ctx.airhouse` and `ctx.oltp.tx` reach the broker like every other host
/// call, so a caught failure in either is noted, and under its own name
/// from the closed list rather than `airhouse.other` or `tx.other`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn caught_airhouse_and_oltp_tx_failures_are_noted_by_name() {
    let host = Arc::new(MockHost {
        own_store_error: Some("connection refused".into()),
        ..Default::default()
    });
    let resp = run_on(
        r#"
            export default async (req, ctx) => {
                const seen = [];
                try {
                    await ctx.airhouse.append("visits", [{ visit_id: "v1" }]);
                } catch (e) { seen.push(e.message); }
                try {
                    await ctx.oltp.tx(async (tx) => tx.exec("UPDATE bookings SET seated = true"));
                } catch (e) { seen.push(e.message); }
                return Response.json({ seen });
            };
        "#,
        test_ctx(),
        host.clone(),
    )
    .await
    .expect("the handler caught both failures and returned");
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.body.matches("connection refused").count(),
        2,
        "both calls failed: {}",
        resp.body
    );
    assert_eq!(
        host.host_call_failures(),
        vec![
            ("airhouse.append", "unreachable"),
            ("tx.begin_oltp", "unreachable"),
        ]
    );
}

/// `req` carries the request, not just its body. Asserts the plumbing end
/// to end — `sanitize_request_headers` decides *which* headers get here,
/// this proves the ones that do actually arrive at app code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn req_exposes_method_and_headers_to_the_handler() {
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let host = Arc::new(MockHost::default());
    let result = run(
        r#"export default async function (req) {
                 return { status: 200, body: JSON.stringify({
                   method: req.method,
                   sig: req.headers["x-hub-signature-256"],
                   ct: req.headers["content-type"],
                   body: req.body,
                 }) };
               }"#
        .to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest {
            method: "POST".to_string(),
            headers: std::collections::BTreeMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                ("x-hub-signature-256".to_string(), "sha256=abc".to_string()),
            ]),
            body: br#"{"hello":"world"}"#.to_vec(),
        },
        host.clone(),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
    .expect("handler must complete");

    let parsed: serde_json::Value = serde_json::from_str(&result.body).unwrap();
    assert_eq!(parsed["method"], "POST");
    assert_eq!(parsed["ct"], "application/json");
    // The whole point: a webhook signature reaches the handler, so it can
    // be verified rather than trusted.
    assert_eq!(parsed["sig"], "sha256=abc");
    // And the body is unchanged by any of this.
    assert_eq!(parsed["body"], r#"{"hello":"world"}"#);
}

/// The published HMAC-SHA256 vector for key "key" over "The quick brown fox
/// jumps over the lazy dog". Using a known-outside vector rather than
/// round-tripping our own output means a wrong implementation cannot agree
/// with itself and pass.
const FOX_HMAC_HEX: &str = "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crypto_hmac_matches_the_published_vector_and_verifies() {
    let (result, _) = run_with_mock(&format!(
            r#"export default async function (req, ctx) {{
                 const key = "key";
                 const data = "The quick brown fox jumps over the lazy dog";
                 return {{ status: 200, body: JSON.stringify({{
                   hex: ctx.crypto.hmac({{ key, data }}),
                   b64: ctx.crypto.hmac({{ key, data, encoding: "base64" }}),
                   good: ctx.crypto.verifyHmac({{ key, data, signature: "{FOX_HMAC_HEX}" }}),
                   tampered: ctx.crypto.verifyHmac({{ key, data, signature: "{tampered}" }}),
                   wrongKey: ctx.crypto.verifyHmac({{ key: "kex", data, signature: "{FOX_HMAC_HEX}" }}),
                 }}) }};
               }}"#,
            FOX_HMAC_HEX = FOX_HMAC_HEX,
            // Same length, one nibble changed — so a length check cannot be
            // what rejects it.
            tampered = format!("e{}", &FOX_HMAC_HEX[1..]),
        ))
        .await;
    let parsed: serde_json::Value =
        serde_json::from_str(&result.expect("handler must complete").body).unwrap();
    assert_eq!(parsed["hex"], FOX_HMAC_HEX);
    // base64 of the SAME published digest, derived from the hex vector
    // independently — not copied from what this code emitted.
    assert_eq!(
        parsed["b64"],
        "97yD9DBThCSxMpjmqm+xQ+9NWaFJRhdZl0edvC0aPNg="
    );
    assert_eq!(parsed["good"], true);
    assert_eq!(parsed["tampered"], false);
    assert_eq!(parsed["wrongKey"], false);
}

/// The security-relevant split: a malformed signature is attacker-controlled
/// and must reject cleanly, while a bad algorithm is an author bug and must
/// throw. Getting this backwards turns a forged request into a 500.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crypto_rejects_junk_signatures_but_throws_on_author_error() {
    let (result, _) = run_with_mock(
        r#"export default async function (req, ctx) {
                 const base = { key: "k", data: "d" };
                 let threw = false;
                 try { ctx.crypto.verifyHmac({ ...base, algorithm: "md5", signature: "aa" }); }
                 catch { threw = true; }
                 return { status: 200, body: JSON.stringify({
                   notHex: ctx.crypto.verifyHmac({ ...base, signature: "zzzz" }),
                   empty: ctx.crypto.verifyHmac({ ...base, signature: "" }),
                   badAlgorithmThrew: threw,
                   eqSame: ctx.crypto.timingSafeEqual("s3cret", "s3cret"),
                   eqDiff: ctx.crypto.timingSafeEqual("s3cret", "s3cres"),
                   eqLen: ctx.crypto.timingSafeEqual("s3cret", "s3cre"),
                 }) };
               }"#,
    )
    .await;
    let parsed: serde_json::Value =
        serde_json::from_str(&result.expect("handler must complete").body).unwrap();
    assert_eq!(
        parsed["notHex"], false,
        "undecodable signature must not throw"
    );
    assert_eq!(parsed["empty"], false);
    assert_eq!(parsed["badAlgorithmThrew"], true);
    assert_eq!(parsed["eqSame"], true);
    assert_eq!(parsed["eqDiff"], false);
    assert_eq!(parsed["eqLen"], false);
}

/// Regression: an unset secret must not authorize. The documented pattern is
/// `timingSafeEqual(req.headers[...], ctx.env.SECRET)`; if the env var was
/// never set and the attacker simply omits the header, both sides coerced to
/// "" and an empty-vs-empty compare returned TRUE. Reported in review.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unset_secret_never_authorizes() {
    let (result, _) = run_with_mock(
            r#"export default async function (req, ctx) {
                 const out = { threw: [] };
                 const t = (name, fn) => { try { out[name] = fn(); } catch { out.threw.push(name); } };

                 // The documented shared-secret pattern, with NOTHING configured
                 // and NOTHING sent. Must not come back true.
                 t("unsetVsMissing", () =>
                   ctx.crypto.timingSafeEqual(req.headers["x-shared-secret"], ctx.env.NOPE));
                 t("bothEmpty", () => ctx.crypto.timingSafeEqual("", ""));
                 t("emptyVsReal", () => ctx.crypto.timingSafeEqual("", "s3cret"));

                 // A missing key must not silently become the literal "undefined",
                 // which is a publicly known key anyone can sign with.
                 t("verifyNoKey", () =>
                   ctx.crypto.verifyHmac({ key: ctx.env.NOPE, data: "d", signature: "aa" }));
                 t("verifyEmptyKey", () =>
                   ctx.crypto.verifyHmac({ key: "", data: "d", signature: "aa" }));
                 t("hmacNoKey", () => ctx.crypto.hmac({ key: ctx.env.NOPE, data: "d" }));

                 return { status: 200, body: JSON.stringify(out) };
               }"#,
        )
        .await;
    let parsed: serde_json::Value =
        serde_json::from_str(&result.expect("handler must complete").body).unwrap();

    assert_eq!(
        parsed["unsetVsMissing"], false,
        "an unset secret + an absent header must NOT authorize"
    );
    assert_eq!(parsed["bothEmpty"], false);
    assert_eq!(parsed["emptyVsReal"], false);

    // A missing/empty key is an author error, so it throws rather than
    // signing with a guessable key.
    let threw: Vec<String> = serde_json::from_value(parsed["threw"].clone()).unwrap();
    for case in ["verifyNoKey", "verifyEmptyKey", "hmacNoKey"] {
        assert!(
            threw.contains(&case.to_string()),
            "{case} must throw: {parsed}"
        );
    }
}

/// The whole point of this and the `req.headers` work together: a GitHub
/// webhook can be verified inside a function, which was impossible before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_github_style_webhook_can_be_verified_end_to_end() {
    let secret = "It's a Secret to Everybody";
    let body = "Hello, World!";
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let host = Arc::new(MockHost::default());
    let result = run(
        r#"export default async function (req, ctx) {
                 const header = req.headers["x-hub-signature-256"] || "";
                 const sig = header.replace(/^sha256=/, "");
                 return { status: 200, body: JSON.stringify({
                   ok: ctx.crypto.verifyHmac({
                     key: ctx.env.WEBHOOK_SECRET, data: req.body, signature: sig,
                   }),
                 }) };
               }"#
        .to_string(),
        uuid::Uuid::new_v4(),
        {
            let mut c = test_ctx();
            c.env
                .insert("WEBHOOK_SECRET".to_string(), secret.to_string());
            c
        },
        FnRequest {
            method: "POST".to_string(),
            headers: std::collections::BTreeMap::from([(
                "x-hub-signature-256".to_string(),
                format!(
                    "sha256={}",
                    hex::encode(super::ops::hmac_digest("sha256", secret, body).unwrap())
                ),
            )]),
            body: body.as_bytes().to_vec(),
        },
        host.clone(),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
    .expect("handler must complete");
    let parsed: serde_json::Value = serde_json::from_str(&result.body).unwrap();
    assert_eq!(parsed["ok"], true, "a valid GitHub signature must verify");
}

/// The happy path of the `ctx.tx` bracket: begin → the author's statements
/// → commit, with the callback's return value handed back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_tx_commits_when_the_callback_resolves() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const id = await ctx.tx("appdb", async (tx) => {
                    const rows = await tx.query("INSERT INTO orders DEFAULT VALUES RETURNING id");
                    await tx.exec("UPDATE inventory SET on_hand = on_hand - $1", [2]);
                    return rows[0].id;
                });
                return Response.json({ id });
            };
        "#,
    )
    .await;

    let resp = result.expect("ctx.tx must resolve");
    assert!(resp.body.contains(r#""id":42"#), "{}", resp.body);
    assert_eq!(
        host.tx_ops(),
        vec!["begin", "query", "exec", "commit"],
        "the wrapper must commit exactly once, after the author's statements"
    );
}

/// The property the whole bracket exists for: an author who throws — or
/// whose statement fails — must not leave a transaction open, and must
/// still see their own error rather than a rollback error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_tx_rolls_back_when_the_callback_throws_and_rethrows_the_original() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                let seen = null;
                try {
                    await ctx.tx("appdb", async (tx) => {
                        await tx.exec("INSERT INTO orders DEFAULT VALUES");
                        throw new Error("inventory went negative");
                    });
                } catch (e) {
                    seen = e.message;
                }
                return Response.json({ seen });
            };
        "#,
    )
    .await;

    let resp = result.expect("the handler itself must still return");
    assert!(
        resp.body.contains("inventory went negative"),
        "the author's error must survive the rollback: {}",
        resp.body
    );
    assert_eq!(
        host.tx_ops(),
        vec!["begin", "exec", "rollback"],
        "a throwing callback must roll back and must NOT commit"
    );
}

/// A handle that escapes its callback must fail loudly. Without this, a
/// stashed handle would address whatever transaction later holds that id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tx_handle_used_after_the_callback_returns_is_rejected() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                let escaped;
                await ctx.tx("appdb", async (tx) => { escaped = tx; });
                let threw = false;
                try { await escaped.exec("DELETE FROM orders"); } catch (e) { threw = true; }
                return Response.json({ threw });
            };
        "#,
    )
    .await;

    let resp = result.expect("handler must return");
    assert!(resp.body.contains(r#""threw":true"#), "{}", resp.body);
    assert_eq!(
        host.tx_ops(),
        vec!["begin", "commit"],
        "the escaped handle must never reach the host"
    );
}

/// Parameters must cross the boundary as a JSON array. If they arrived
/// stringified, binding would silently become interpolation — the exact
/// failure this API exists to prevent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tx_params_cross_the_boundary_as_an_array() {
    let (result, _host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const rows = await ctx.tx("appdb", (tx) =>
                    tx.query("SELECT * FROM t WHERE a = $1 AND b = $2", [7, "x"]));
                return Response.json({ params: rows[0].params });
            };
        "#,
    )
    .await;

    let resp = result.expect("handler must return");
    assert!(
        resp.body.contains(r#""params":[7,"x"]"#),
        "params must arrive as an array: {}",
        resp.body
    );
}

/// Omitting `params` is the common case (a statement with no placeholders)
/// and must not become `undefined` on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tx_params_default_to_an_empty_array() {
    let (result, _host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const rows = await ctx.tx("appdb", (tx) => tx.query("SELECT 1"));
                return Response.json({ params: rows[0].params });
            };
        "#,
    )
    .await;

    let resp = result.expect("handler must return");
    assert!(resp.body.contains(r#""params":[]"#), "{}", resp.body);
}

/// Regression: a handler that awaits an async host op (`ctx.query`) must
/// RESUME and return once the op replies — it must not hang until the
/// wall-clock timeout. This reproduces the `resolve()`-doesn't-pump-the-
/// event-loop bug: with the buggy invoke path this returns `Err(Timeout)`;
/// with `with_event_loop_promise` it returns the handler's `Response` fast.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handler_resumes_after_async_host_op() {
    let artifact = r#"
            export default async (req, ctx) => {
                const r = await ctx.query("select 1");
                return Response.json({ rows: r.rows.length });
            };
        "#;
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let result = run(
        artifact.to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        Arc::new(MockHost::default()),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await;
    let resp = result.expect("handler must resume after the async host op, not time out");
    assert!(
        resp.body.contains("\"rows\":1"),
        "unexpected handler body: {}",
        resp.body
    );
}

/// `ctx.warehouse.query` reaches a named database.
///
/// `ctx.query` only ever hits the project default, and every other
/// named-database surface is a write — so before this an app had no way to
/// read its own OLTP store unless it happened to be the default.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warehouse_query_reads_a_named_database() {
    let artifact = r#"
            export default async (req, ctx) => {
                const r = await ctx.warehouse.query("oltp", "SELECT 1");
                return Response.json({ rows: r.rows.length, db: r.db });
            };
        "#;
    let host = Arc::new(MockHost::default());
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let resp = run(
        artifact.to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        host.clone(),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
    .expect("handler should succeed");

    assert!(resp.body.contains("\"rows\":1"), "body: {}", resp.body);
    // The database name has to survive the boundary, or the read would
    // silently target whatever the host defaulted to.
    assert!(resp.body.contains("\"db\":\"oltp\""), "body: {}", resp.body);
}

/// `ctx.oltp.query(sql, params)` reaches `host.oltp("query", { sql, params })`
/// with `params` as an array — the whole JS → op → HostCall → dispatch wiring
/// for the app-writer path. No database name crosses the boundary: the app's
/// own writer is derived host-side from its slug (the manifest only gates).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oltp_query_forwards_sql_and_params_to_the_host() {
    let (result, _host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const rows = await ctx.oltp.query(
                    "SELECT * FROM bookings WHERE party_size > $1", [4]);
                return Response.json(rows[0]);
            };
        "#,
    )
    .await;

    let resp = result.expect("handler must return");
    assert!(
        resp.body.contains(r#""params":[4]"#),
        "params must cross as an array, not a stringified blob: {}",
        resp.body
    );
    assert!(
        resp.body.contains("party_size"),
        "sql must cross the boundary intact: {}",
        resp.body
    );
}

/// `ctx.oltp.tx(fn)` opens with `begin_oltp` — no database crosses the
/// boundary, the writer is the host's to derive — and closes through the
/// same commit bracket as `ctx.tx`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oltp_tx_opens_on_the_apps_own_store_and_commits() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const n = await ctx.oltp.tx(async (tx) => {
                    await tx.exec("UPDATE bookings SET seated = true WHERE id = $1", [7]);
                    return tx.exec("INSERT INTO seatings (booking_id) VALUES ($1)", [7]);
                });
                return Response.json({ n });
            };
        "#,
    )
    .await;

    let resp = result.expect("ctx.oltp.tx must resolve");
    assert!(resp.body.contains(r#""n":1"#), "body: {}", resp.body);
    assert_eq!(host.tx_ops(), vec!["begin_oltp", "exec", "exec", "commit"]);
}

/// `ctx.airhouse.append(table, rows)` sends a bare table and the rows as an
/// array; adding the schema is the host's job, not the script's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn airhouse_append_forwards_the_table_and_rows() {
    let (result, _host) = run_with_mock(
            r#"
            export default async (req, ctx) => {
                const n = await ctx.airhouse.append("visits", [{ visit_id: "v1" }, { visit_id: "v2" }]);
                return Response.json({ n, schema: ctx.airhouse.schema });
            };
        "#,
        )
        .await;

    let resp = result.expect("ctx.airhouse.append must resolve");
    assert!(resp.body.contains(r#""n":2"#), "body: {}", resp.body);
    assert!(
        resp.body.contains(r#""schema":null"#),
        "no capability, no schema: {}",
        resp.body
    );
}

/// `ctx.oltp.exec` omitting `params` sends `[]`, not `undefined` — the same
/// guarantee `ctx.tx` makes, so a no-placeholder write is not read as a
/// wrong-arity error host-side.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oltp_exec_returns_row_count() {
    let (result, _host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const n = await ctx.oltp.exec("DELETE FROM bookings WHERE cancelled");
                return Response.json({ n });
            };
        "#,
    )
    .await;

    let resp = result.expect("handler must return");
    assert!(resp.body.contains(r#""n":1"#), "body: {}", resp.body);
}

/// Regression: this isolate is bare `deno_core` (no `deno_web`) on a V8 that
/// predates `Uint8Array.prototype.toBase64`, so `btoa` was `undefined` — an
/// author could not produce the base64 that `ctx.email.send` attachments
/// require, and attaching a generated file was impossible. Proves the
/// encoder exists AND that the bytes reach the host unmangled.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn isolate_can_base64_encode_attachment_bytes() {
    let artifact = r#"
            export default async (req, ctx) => {
                // Latin1 bytes as a string — what btoa is actually specified for.
                const pdf = "\x25\x50\x44\x46"; // "%PDF"
                await ctx.email.send({
                    to: "a@b.com",
                    subject: "report",
                    text: "see attached",
                    attachments: [{ filename: "r.pdf", content: btoa(pdf) }],
                });
                let wideThrew = false;
                try { btoa("日本語"); } catch { wideThrew = true; }
                // Handing btoa raw bytes must FAIL rather than encode
                // String(u8) == "37,80,68,70". Silent corruption is the thing
                // this whole change exists to prevent.
                let bytesThrew = false;
                try { btoa(new Uint8Array([0x25, 0x50])); } catch { bytesThrew = true; }
                // Padding must not terminate the decode early: concatenated
                // base64 is malformed and has to say so, not truncate.
                let concatThrew = false;
                try { atob(btoa("hello") + btoa("world")); } catch { concatThrew = true; }
                return Response.json({
                    str: btoa("hello"),
                    roundTrip: atob(btoa("hello")),
                    wideThrew,
                    bytesThrew,
                    concatThrew,
                });
            };
        "#;
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let host = Arc::new(MockHost::default());
    let resp = run(
        artifact.to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        host.clone(),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
    .expect("btoa/atob must exist in the isolate");

    assert!(resp.body.contains(r#""str":"aGVsbG8=""#), "{}", resp.body);
    assert!(
        resp.body.contains(r#""roundTrip":"hello""#),
        "{}",
        resp.body
    );
    // Latin1-only, exactly like a browser. Note the subtler trap this
    // guards: btoa ACCEPTS U+0080..U+00FF and encodes them as Latin1, so
    // `btoa(csv)` on accented text yields mojibake rather than an error —
    // which is why generated text should use `encoding: "utf8"` instead.
    assert!(resp.body.contains(r#""wideThrew":true"#), "{}", resp.body);
    assert!(resp.body.contains(r#""bytesThrew":true"#), "{}", resp.body);
    assert!(resp.body.contains(r#""concatThrew":true"#), "{}", resp.body);

    let email = host
        .last_email
        .lock()
        .unwrap()
        .clone()
        .expect("ctx.email.send must reach the host");
    assert_eq!(email["attachments"][0]["content"], "JVBERg==");
}

/// The other half of the fix: generated TEXT needs no encoder at all, and
/// `encoding: "utf8"` must survive the isolate boundary so the host can
/// attach the bytes verbatim.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn utf8_attachment_crosses_the_boundary_intact() {
    let artifact = r#"
            export default async (req, ctx) => {
                await ctx.email.send({
                    to: "a@b.com",
                    subject: "report",
                    text: "see attached",
                    attachments: [{
                        filename: "report.csv",
                        content: "name,total\nCafé,3\n",
                        encoding: "utf8",
                        contentType: "text/csv",
                    }],
                });
                return Response.json({ ok: true });
            };
        "#;
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let host = Arc::new(MockHost::default());
    run(
        artifact.to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        host.clone(),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
    .expect("handler must complete");

    let email = host.last_email.lock().unwrap().clone().expect("sent");
    let att = &email["attachments"][0];
    assert_eq!(att["encoding"], "utf8");
    assert_eq!(att["content"], "name,total\nCafé,3\n");
}

/// `ctx.notify` exists in the isolate, hands the host the payload as
/// written — targets, kind, and the email block — and resolves to what the
/// host answered.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn notify_reaches_the_host_with_its_payload() {
    let artifact = r#"
            export default async (req, ctx) => {
                const r = await ctx.notify({
                    to: [{ role: "Store Manager", location: "6f1c2a8e-1b7a-4c35-9d55-0c3f1f2b9a10" }],
                    kind: "list_overdue",
                    title: "Opening list is late",
                    link: "/lists/42",
                    email: true,
                    idempotencyKey: "overdue:42",
                });
                return Response.json({ id: r.notificationId, recipients: r.recipients });
            };
        "#;
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let host = Arc::new(MockHost::default());
    let resp = run(
        artifact.to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        host.clone(),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
    .expect("handler must complete");

    assert!(resp.body.contains(r#""id":"n-1""#), "{}", resp.body);
    assert!(resp.body.contains(r#""recipients":2"#), "{}", resp.body);
    let sent = host.last_notify.lock().unwrap().clone().expect("notified");
    assert_eq!(sent["to"][0]["role"], "Store Manager");
    assert_eq!(sent["kind"], "list_overdue");
    assert_eq!(sent["email"], true);
    assert_eq!(sent["idempotencyKey"], "overdue:42");
}

// ── Invocation meters ────────────────────────────────────────────────

/// The distinction the `Option` exists for. An invocation that never
/// reached tenant code — a module that failed to compile, a timeout during
/// setup — must not report that setup took 0 ms, which would read as
/// "instant" and hide exactly the case worth seeing.
#[test]
fn init_ms_is_absent_until_tenant_code_is_reached_not_zero() {
    let meters = InvocationMeters::start();
    assert_eq!(meters.init_ms(), None, "unmarked must be absent, not 0");

    meters.mark_tenant_code_entered();
    // Elapsed can legitimately round to 0 ms on a fast machine; what this
    // pins is that the *stored* value is no longer the sentinel.
    meters
        .init_ms
        .fetch_max(1, std::sync::atomic::Ordering::Relaxed);
    assert!(meters.init_ms().is_some(), "marked must be present");
}

/// The counter is a shared handle precisely because the isolate thread can
/// outlive `run`; a clone must observe the same count, or the caller reads
/// zero for a function that made a hundred calls.
#[test]
fn host_call_count_is_shared_across_clones() {
    let meters = InvocationMeters::start();
    let thread_side = meters.clone();
    thread_side
        .host_calls
        .fetch_add(3, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(meters.host_calls(), 3);
}

/// Healthy is zero, and the accessor has to read the same counter the
/// grace arm increments — a gauge wired to the wrong static reports calm
/// forever, which is the failure it exists to catch.
#[test]
fn the_abandoned_isolate_gauge_starts_at_zero_and_observes_increments() {
    let before = abandoned_isolates();
    let reported = oxy_telemetry::metrics::sources::isolate_abandoned();
    assert_eq!(
        reported,
        before + 1,
        "the incrementer returns the new total"
    );
    assert_eq!(
        abandoned_isolates(),
        before + 1,
        "the accessor must read the same counter the grace arm increments"
    );
    oxy_telemetry::metrics::sources::ISOLATES_ABANDONED
        .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
}

/// The live gauge has to come back to where it started once the guard is
/// dropped. A drifting gauge is worse than no gauge: it is the number a
/// concurrency cap would be sized from.
#[test]
fn the_isolate_guard_balances() {
    use oxy_telemetry::metrics::sources::{ISOLATES_LIVE, IsolateGuard};
    use std::sync::atomic::Ordering;

    let before = ISOLATES_LIVE.load(Ordering::Relaxed);
    {
        let _a = IsolateGuard::enter();
        let _b = IsolateGuard::enter();
        assert_eq!(ISOLATES_LIVE.load(Ordering::Relaxed), before + 2);
    }
    assert_eq!(ISOLATES_LIVE.load(Ordering::Relaxed), before);
}

/// An op that reaches an isolate before an invocation is bound is refused with
/// the dropped-broker text; it must not panic the isolate thread.
#[test]
fn an_op_with_no_invocation_bound_is_refused_not_panicked() {
    let refusal = super::ops::host_sender_of(None).err().expect("refused");
    assert_eq!(refusal.to_string(), "function host unavailable");
}

/// Each call's trace record, from a real run against this file's host.
#[path = "call_trace_tests.rs"]
mod call_trace_tests;
