//! The broker's gates: capability, environment, registry.

use super::*;

// ── The broker's gates: capability, environment, registry ───────────────

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
