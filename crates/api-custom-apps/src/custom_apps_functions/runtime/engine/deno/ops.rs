//! The `#[op2]` bindings for the `ctx.*` ops, and the `oxy_functions_ext`
//! extension that registers them.
//!
//! Each op is glue only: unpack `deno_core`'s arguments, call the
//! engine-neutral function in `runtime::ops`, and turn an `OpError` into a
//! `JsErrorBox::generic` (a plain JS `Error` carrying the same message). The
//! op names are the `Deno.core.ops.*` names `BOOTSTRAP_JS` calls, so they are
//! a contract with it.
//!
//! An async op takes its [`HostSender`] in its own statement so the
//! `OpState` borrow is released before the op awaits the broker.

use std::cell::RefCell;
use std::rc::Rc;

use deno_core::{OpState, op2};
use deno_error::JsErrorBox;

use crate::custom_apps_functions::runtime::host_call::HostCall;
use crate::custom_apps_functions::runtime::ops::{
    self as neutral, HostSender, OpContext, OpError, RequestBody,
};

impl From<OpError> for JsErrorBox {
    fn from(e: OpError) -> Self {
        JsErrorBox::generic(e.0)
    }
}

/// The cancellation check, then a handle on the broker channel.
fn host_sender(state: &Rc<RefCell<OpState>>) -> Result<HostSender, OpError> {
    neutral::host_sender_of(state.borrow().try_borrow::<OpContext>())
}

// Still `(fast)`: the JS-visible args are primitives (no return), so deno_core
// 0.331 requires `(fast)` — and a fast op may take `&mut OpState` as a leading
// special arg to reach the shared log buffer.
#[op2(fast)]
fn op_ctx_log(state: &mut OpState, #[string] level: &str, #[string] message: &str) {
    // Absent context: drop the line rather than emit it uncapped. Reachable only
    // before `bind_invocation` parks one.
    if let Some(ctx) = state.try_borrow::<OpContext>() {
        ctx.log(level, message);
    }
}

#[op2]
#[string]
async fn op_ctx_query(
    state: Rc<RefCell<OpState>>,
    #[string] sql: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    Ok(host
        .call("ctx.query", |reply| HostCall::Query { sql, reply })
        .await?)
}

#[op2]
#[string]
async fn op_ctx_query_stream(
    state: Rc<RefCell<OpState>>,
    #[string] sql: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    Ok(host
        .call("ctx.queryStream", |reply| HostCall::QueryStream {
            sql,
            reply,
        })
        .await?)
}

#[op2]
#[string]
async fn op_ctx_fetch(
    state: Rc<RefCell<OpState>>,
    #[string] url: String,
    #[string] init_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let init = neutral::parse_payload(&init_json);
    Ok(host
        .call("ctx.fetch", |reply| HostCall::Fetch { url, init, reply })
        .await?)
}

#[op2]
#[string]
async fn op_ctx_warehouse(
    state: Rc<RefCell<OpState>>,
    #[string] op: String,
    #[string] payload_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let payload = neutral::parse_payload(&payload_json);
    Ok(host
        .call("ctx.warehouse", |reply| HostCall::WarehouseWrite {
            op,
            payload,
            reply,
        })
        .await?)
}

/// `ctx.tx` — bridge to `FunctionHost::tx`.
///
/// One op for all five verbs (`begin`/`query`/`exec`/`commit`/`rollback`), same
/// as `op_ctx_warehouse`. The transaction handle never crosses this boundary —
/// the isolate only ever holds the integer id `begin` returns, so a script
/// cannot fabricate a connection, only name one it was given.
#[op2]
#[string]
async fn op_ctx_tx(
    state: Rc<RefCell<OpState>>,
    #[string] op: String,
    #[string] payload_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let payload = neutral::parse_payload(&payload_json);
    Ok(host
        .call("ctx.tx", |reply| HostCall::Tx { op, payload, reply })
        .await?)
}

/// `ctx.oltp.{query,exec}` — bridge to `FunctionHost::oltp`. Single-op
/// dispatcher shaped like `op_ctx_warehouse`: `op` is the verb, `payload_json`
/// carries `{ sql, params? }`. The app's own per-org OLTP schema is derived
/// host-side from the invoking app's slug (the manifest only gates), so no
/// database name — and no app-chosen target — crosses this boundary.
#[op2]
#[string]
async fn op_ctx_oltp(
    state: Rc<RefCell<OpState>>,
    #[string] op: String,
    #[string] payload_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let payload = neutral::parse_payload(&payload_json);
    Ok(host
        .call("ctx.oltp", |reply| HostCall::Oltp { op, payload, reply })
        .await?)
}

/// `ctx.airhouse.{query,exec,append}` — bridge to `FunctionHost::airhouse`.
/// Shaped like `op_ctx_oltp`: the app's schema is derived host-side from its
/// slug, so no schema or database name crosses this boundary.
#[op2]
#[string]
async fn op_ctx_airhouse(
    state: Rc<RefCell<OpState>>,
    #[string] op: String,
    #[string] payload_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let payload = neutral::parse_payload(&payload_json);
    Ok(host
        .call("ctx.airhouse", |reply| HostCall::Airhouse {
            op,
            payload,
            reply,
        })
        .await?)
}

/// `ctx.events.publish(type, data, opts)` — bridge to `FunctionHost::events`.
/// The app is the host's own, so no app or org id crosses this boundary.
#[op2]
#[string]
async fn op_ctx_events(
    state: Rc<RefCell<OpState>>,
    #[string] op: String,
    #[string] payload_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let payload = neutral::parse_payload(&payload_json);
    Ok(host
        .call("ctx.events", |reply| HostCall::Events {
            op,
            payload,
            reply,
        })
        .await?)
}

/// `ctx.apps.call` / `ctx.apps.send` — bridge to `FunctionHost::apps`. The
/// calling app is the host's own, so no caller identity crosses this boundary.
#[op2]
#[string]
async fn op_ctx_apps(
    state: Rc<RefCell<OpState>>,
    #[string] op: String,
    #[string] payload_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let payload = neutral::parse_payload(&payload_json);
    Ok(host
        .call("ctx.apps", |reply| HostCall::Apps { op, payload, reply })
        .await?)
}

/// `ctx.secrets.set(key, value)` — bridge to `FunctionHost::secrets_set`.
#[op2]
#[string]
async fn op_ctx_secrets_set(
    state: Rc<RefCell<OpState>>,
    #[string] key: String,
    #[string] value: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    Ok(host
        .call("ctx.secrets.set", |reply| HostCall::SecretsSet {
            key,
            value,
            reply,
        })
        .await?)
}

/// `ctx.email.send(input)` — bridge to `FunctionHost::send_email`. `input` is
/// the JS payload object, JSON-stringified by the bootstrap `__wrapOp`.
#[op2]
#[string]
async fn op_ctx_email_send(
    state: Rc<RefCell<OpState>>,
    #[string] input_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let input = neutral::parse_payload(&input_json);
    Ok(host
        .call("ctx.email.send", |reply| HostCall::SendEmail {
            input,
            reply,
        })
        .await?)
}

/// `ctx.notify(input)` — bridge to `FunctionHost::notify`. `input` is the JS
/// payload object, JSON-stringified by the bootstrap `__wrapOp`.
#[op2]
#[string]
async fn op_ctx_notify(
    state: Rc<RefCell<OpState>>,
    #[string] input_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let input = neutral::parse_payload(&input_json);
    Ok(host
        .call("ctx.notify", |reply| HostCall::Notify { input, reply })
        .await?)
}

/// `ctx.storage.*` — bridge to `FunctionHost::storage`. `op` selects the
/// operation ("getUploadUrl" / "getDownloadUrl" / "put" / "get" / "head" /
/// "list" / "delete" / "copy"), `payload` carries its args (JSON-stringified by
/// `__wrapOp`).
#[op2]
#[string]
async fn op_ctx_storage(
    state: Rc<RefCell<OpState>>,
    #[string] op: String,
    #[string] payload_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let payload = neutral::parse_payload(&payload_json);
    Ok(host
        .call("ctx.storage", |reply| HostCall::Storage {
            op,
            payload,
            reply,
        })
        .await?)
}

// The three `ctx.org.*` ops take no payload: the org comes from the host. A
// function that could name its own org here would be a manifest pointing at
// another tenant's roster.

#[op2]
#[string]
async fn op_ctx_org_people(state: Rc<RefCell<OpState>>) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    Ok(host
        .call("ctx.org.people", |reply| HostCall::OrgPeople { reply })
        .await?)
}

#[op2]
#[string]
async fn op_ctx_org_places(state: Rc<RefCell<OpState>>) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    Ok(host
        .call("ctx.org.places", |reply| HostCall::OrgPlaces { reply })
        .await?)
}

#[op2]
#[string]
async fn op_ctx_org_assignments(state: Rc<RefCell<OpState>>) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    Ok(host
        .call("ctx.org.assignments", |reply| HostCall::OrgAssignments {
            reply,
        })
        .await?)
}

/// `ctx.platform.facts(opts)` — bridge to `FunctionHost::platform_facts`.
/// `opts` (JSON-stringified by `__wrapOp`) names a span; which org is asking
/// is the host's to know, so nothing here can name one.
#[op2]
#[string]
async fn op_ctx_platform_facts(
    state: Rc<RefCell<OpState>>,
    #[string] opts_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let opts = neutral::parse_payload(&opts_json);
    Ok(host
        .call("ctx.platform.facts", |reply| HostCall::PlatformFacts {
            opts,
            reply,
        })
        .await?)
}

/// `ctx.oxygen.*` — bridge to `FunctionHost::app_action`. `op` is the
/// sub-op (`frontline.devices.list`, `sourceUploads.report`); an unknown one is
/// refused by the broker as an unknown op. Nothing here names an org.
#[op2]
#[string]
async fn op_ctx_oxygen(
    state: Rc<RefCell<OpState>>,
    #[string] op: String,
    #[string] payload_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let payload = neutral::parse_payload(&payload_json);
    Ok(host
        .call("ctx.oxygen", |reply| HostCall::AppAction {
            op,
            payload,
            reply,
        })
        .await?)
}

#[op2]
#[string]
async fn op_ctx_semantic_query(
    state: Rc<RefCell<OpState>>,
    #[string] spec_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let spec = neutral::parse_semantic_spec(&spec_json)?;
    Ok(host
        .call("ctx.semantic.query", |reply| HostCall::SemanticQuery {
            spec,
            reply,
        })
        .await?)
}

#[op2]
#[string]
async fn op_ctx_airway_run(
    state: Rc<RefCell<OpState>>,
    #[string] pipeline_ref: String,
    #[string] vars_json: String,
) -> Result<String, JsErrorBox> {
    let host = host_sender(&state)?;
    let variables = neutral::parse_payload(&vars_json);
    Ok(host
        .call("ctx.airway.run", |reply| HostCall::AirwayRun {
            pipeline_ref,
            variables,
            reply,
        })
        .await?)
}

/// `ctx.crypto.hmac` — see `runtime::ops::hmac`.
#[op2]
#[string]
fn op_ctx_hmac(
    #[string] algorithm: &str,
    #[string] key: &str,
    #[string] data: &str,
    #[string] encoding: &str,
) -> Result<String, JsErrorBox> {
    Ok(neutral::hmac(algorithm, key, data, encoding)?)
}

/// `ctx.crypto.verifyHmac` — see `runtime::ops::verify_hmac`.
#[op2(fast)]
fn op_ctx_verify_hmac(
    #[string] algorithm: &str,
    #[string] key: &str,
    #[string] data: &str,
    #[string] signature: &str,
    #[string] encoding: &str,
) -> Result<bool, JsErrorBox> {
    Ok(neutral::verify_hmac(
        algorithm, key, data, signature, encoding,
    )?)
}

/// `ctx.crypto.timingSafeEqual` — see `runtime::ops::timing_safe_equal`.
#[op2(fast)]
fn op_ctx_timing_safe_equal(#[string] a: &str, #[string] b: &str) -> bool {
    neutral::timing_safe_equal(a, b)
}

/// Hand the request body to the invoke script, once; any later call gets an
/// empty string. See `runtime::ops::RequestBody`.
#[op2]
#[string]
fn op_req_take_body(state: &mut OpState) -> String {
    state
        .try_take::<RequestBody>()
        .map(|body| body.0)
        .unwrap_or_default()
}

deno_core::extension!(
    oxy_functions_ext,
    ops = [
        op_req_take_body,
        op_ctx_log,
        op_ctx_query,
        op_ctx_query_stream,
        op_ctx_fetch,
        op_ctx_warehouse,
        op_ctx_secrets_set,
        op_ctx_semantic_query,
        op_ctx_airway_run,
        op_ctx_email_send,
        op_ctx_notify,
        op_ctx_storage,
        op_ctx_org_people,
        op_ctx_org_places,
        op_ctx_org_assignments,
        op_ctx_platform_facts,
        op_ctx_oxygen,
        op_ctx_tx,
        op_ctx_oltp,
        op_ctx_airhouse,
        op_ctx_events,
        op_ctx_apps,
        op_ctx_hmac,
        op_ctx_verify_hmac,
        op_ctx_timing_safe_equal,
    ],
);
