//! V8-isolate execution for Oxy Functions.
//!
//! See `internal-docs/customer-apps-functions.md` §4 and
//! §11. Given a bundled function artifact (esbuild ESM output) and a `ctx`
//! payload, run `export default async (req, ctx) => Response` to completion
//! and return the resulting status/body.
//!
//! ## Why a dedicated thread + channel broker
//!
//! `deno_core::JsRuntime` owns a V8 isolate and is `!Send` — it cannot be
//! held across `.await` in the `Send` axum handler future, nor moved between
//! tokio workers. So the isolate runs on its **own OS thread** with a
//! current-thread tokio runtime, and every `ctx.*` host call is bridged to
//! the async host side (DB connectors, outbound fetch) over an mpsc channel.
//! The handler future only ever holds channel endpoints + a join handle, all
//! of which are `Send`.
//!
//! ```text
//!   handler (Send)            isolate thread (!Send)
//!   ─────────────             ──────────────────────
//!   broker loop  <── HostCall ── op_ctx_query / op_ctx_fetch
//!        │  host.query(sql).await
//!        └────────── oneshot reply ─────────►  resolves the JS promise
//! ```
//!
//! `ctx.semantic.query` (airlayer), `ctx.airway.run` (Airway runner), and
//! `ctx.warehouse.{insert,exec,upsert}` (project-database allowlist, §11.3)
//! are all wired to real backends.
//!
//! `ctx.tx` adds multi-statement atomicity over that same allowlist: the
//! isolate holds only a handle id, the pinned connection lives in the host's
//! `tx::TxRegistry`, and the bootstrap wrapper owns the commit/rollback
//! bracket so an author cannot leave one open. `ctx.queryStream` (§11.5) fetches up to
//! `FUNCTION_STREAM_MAX_ROWS` rows in one host call and yields them to the
//! function as an async generator in client-side batches — a pragmatic MVP
//! pending a true warehouse-cursor implementation.
//!
//! ## Layout
//!
//! This module is the engine-neutral runtime: the types ([`types`]), the
//! host trait ([`function_host`]), the bootstrap script, admission and the
//! broker loop ([`broker`]), the isolate thread ([`isolate_thread`]) and
//! its limits ([`isolate_limits`]). The JS engine sits
//! behind [`engine::IsolateEngine`] (`deno_core` is its only implementation,
//! in `engine/deno`); what each `ctx.*` op does is in [`ops`], and the
//! broker's side of a host call in [`host_call`].

use std::sync::Arc;

use tokio::sync::oneshot;

mod broker;
mod call_label;
#[cfg(test)]
mod call_label_tests;
mod engine;
mod function_host;
mod host_call;
mod host_call_span;
mod host_call_task;
mod isolate_limits;
mod isolate_thread;
mod ops;
#[cfg(test)]
mod snapshot_tests;
#[cfg(test)]
mod terminate_gap_tests;
#[cfg(test)]
mod test_calls;
#[cfg(test)]
mod tests;
mod types;
mod warm_pool;
#[cfg(test)]
mod warm_pool_tests;

use super::LogLine;
use crate::custom_apps_telemetry::InvocationMeters;
#[cfg(test)]
use host_call::{dispatch_host_call, host_call_span, host_call_target};

use broker::run_in;
pub use function_host::FunctionHost;
pub use isolate_limits::{HEAP_LIMIT_MB_ENV, abandoned_isolates, heap_limit_bytes};
use isolate_thread::{IsolateRun, execute_on_engine, spawn_cold};
pub use types::{
    CtxIdentityKind, CtxTeam, CtxUser, FUNCTION_MAX_ROWS, FUNCTION_STREAM_MAX_ROWS, FnRequest,
    FnResponse, InvocationCtx, RuntimeError,
};

/// Bootstrap script: polyfills the minimal `Response` the function author's
/// code expects, and assembles `globalThis.__buildCtx` from the host ops.
///
/// Kept in its own file because the engine builds a startup snapshot from it
/// (`engine::IsolateConfig::bootstrap`): every isolate starts with these
/// definitions already in its global scope instead of parsing and running the
/// script again.
const BOOTSTRAP_JS: &str = include_str!("runtime/bootstrap.js");

/// Run `export default async (req, ctx) => Response` from a bundled ESM
/// artifact to completion, bridging `ctx.*` calls to `host`.
///
/// `cancel` resolving (the client disconnected, or the dashboard cancel
/// flag was observed) terminates the isolate promptly via
/// `terminate_execution`. `timeout` is enforced here (not by the caller
/// wrapping this future in `tokio::time::timeout`): on elapse the isolate is
/// terminated the same way as a cancel, and we wait up to `TIMEOUT_GRACE`
/// for the isolate thread to actually exit before returning — so in the common
/// case the OS thread + V8 isolate never outlive this call. If the isolate is
/// wedged in a not-yet-returned host call (which `terminate_execution` cannot
/// interrupt), we return `Timeout` after the grace period and let that thread
/// unwind on its own once the (individually bounded) host op completes.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    artifact_js: String,
    org_id: uuid::Uuid,
    ctx: InvocationCtx,
    req: FnRequest,
    host: std::sync::Arc<dyn FunctionHost>,
    cancel: oneshot::Receiver<()>,
    timeout: std::time::Duration,
    logs: Arc<std::sync::Mutex<Vec<LogLine>>>,
    meters: InvocationMeters,
    isolate_span: tracing::Span,
) -> Result<FnResponse, RuntimeError> {
    run_in(
        warm_pool::global(),
        artifact_js,
        org_id,
        ctx,
        req,
        host,
        cancel,
        timeout,
        logs,
        meters,
        isolate_span,
    )
    .await
}
