//! The host-side data plane a function's `ctx.*` calls reach — the broker
//! calls these; the isolate never does.

use super::super::env_policy::{EnvPolicy, HeldTarget, HostOp};
use super::super::host::FunctionCapabilities;
use super::super::host_error::HostError;

/// Host-side data plane the isolate calls back into. Implemented in
/// `mod.rs` against the resolved project context (connectors) + an HTTP
/// client. `Send + Sync` so the broker loop can `Arc`-clone it per call.
#[async_trait::async_trait]
pub trait FunctionHost: Send + Sync {
    /// `ctx.query(sql)` — read-only SQL, function-scoped row cap. Returns
    /// the rows as a JSON array value.
    async fn query(&self, sql: String) -> Result<serde_json::Value, HostError>;
    /// Called once after the isolate has finished (or been cancelled), on the
    /// caller's side of the channel: the place for work that must happen once
    /// per invocation rather than once per host call.
    async fn end_of_invocation(&self) {}

    /// What this function's manifest grants. The broker reads it before every
    /// op (`host_call::dispatch_host_call`): an op whose registry row names a
    /// capability the manifest lacks is refused with the host's own words
    /// (`FunctionCapabilities::refuse`) and never dispatched, so a method
    /// below runs only for an op its manifest allows. Fail-closed by default:
    /// a host that does not say grants nothing.
    fn capabilities(&self) -> &FunctionCapabilities {
        static NOTHING: std::sync::LazyLock<FunctionCapabilities> =
            std::sync::LazyLock::new(FunctionCapabilities::default);
        &NOTHING
    }
    /// What each op may do in the environment this run was admitted to. The
    /// broker decides every op that holds by not running (`HoldBy::Broker`)
    /// before it is dispatched; an op that holds by a mechanism of its own
    /// asks this policy inside its method. Production by default — what
    /// every run was before environments — so a test double is never held.
    fn env_policy(&self) -> &EnvPolicy {
        static PRODUCTION: std::sync::LazyLock<EnvPolicy> =
            std::sync::LazyLock::new(EnvPolicy::production);
        &PRODUCTION
    }
    /// Called by the broker when the environment held or refused an op
    /// before dispatch, with the identifiers the held-write log records it
    /// under (never values). The real host writes the `app.staging.held`
    /// row from these.
    async fn record_held(&self, _op: HostOp, _target: HeldTarget<'_>) {}

    /// Whether this host's environment policy decides — outside production,
    /// or a production run refused for reading a branch. Only then is a
    /// `HeldInStaging` / `EnvironmentRefused` label on its errors the
    /// policy's own (`host_call_attrs::classify_host_error_for`).
    fn decides_by_environment(&self) -> bool {
        false
    }
    /// Called by the broker when a host call failed in a way that pages
    /// (`host_call_attrs::counts_toward_paging`), before the isolate sees the
    /// rejection — so a handler that catches it and answers 2xx still leaves a
    /// failure behind. `op` is a fixed op name, never what the call carried;
    /// `message` is the host's error text, which the host normalizes before it
    /// keeps anything (`failure_signal::HostCallFailure::noted`), so the
    /// fingerprint can tell a new break on an op apart from the routine
    /// failure a handler already catches there; `target` is where the call
    /// went, for the ops that name one (`host_call_target`), normalized the
    /// same way before it is kept.
    fn note_host_call_failure(
        &self,
        _op: &'static str,
        _kind: &'static str,
        _message: &str,
        _target: Option<&str>,
    ) {
    }
    /// Called by the broker when a host call succeeded, so a failure noted for
    /// the same `op` against the same `target` earlier in the run is dropped:
    /// the run recovered from it, and the fingerprint should name one it did
    /// not. A success against another target — a `fetch` to Slack after a
    /// failed `fetch` to a vendor — clears nothing. The rule:
    /// `failure_signal::HostCallFailure::recovered`.
    fn note_host_call_success(&self, _op: &'static str, _target: Option<&str>) {}
    /// The failure noted for this invocation — the first not recovered from —
    /// read once the isolate has finished.
    fn host_call_failure(&self) -> Option<super::super::failure_signal::HostCallFailure> {
        None
    }
    /// `ctx.queryStream(sql)` — read-only SQL with a higher row cap
    /// (`FUNCTION_STREAM_MAX_ROWS`) for large scans. Returns the rows as a
    /// JSON array value; the isolate yields them to the function in batches.
    async fn query_stream(&self, sql: String) -> Result<serde_json::Value, HostError>;
    /// `ctx.fetch(url, init)` — SSRF-allowlisted outbound HTTP with a
    /// response size cap. Returns `{ status, body }`.
    async fn fetch(
        &self,
        url: String,
        init: serde_json::Value,
    ) -> Result<serde_json::Value, HostError>;
    /// `ctx.semantic.query(spec)` — airlayer-compiled semantic query.
    /// `spec` is the JSON-encoded `agentic_semantic::config::SemanticQueryConfig`.
    async fn semantic_query(&self, spec: serde_json::Value)
    -> Result<serde_json::Value, HostError>;

    /// `ctx.org.people()` — the org's people directory, read-only.
    ///
    /// Takes no arguments on purpose: the org is the host's, so a function
    /// cannot name one and a manifest cannot point this at another tenant.
    ///
    /// Defaulted rather than required, because every other implementor of this
    /// trait is a test double that has no directory to answer with — and the
    /// default is the same fail-closed refusal the capability gate gives, so a
    /// double that forgets to override it denies rather than inventing people.
    async fn org_people(&self) -> Result<serde_json::Value, HostError> {
        Err(HostError::backend(
            "ctx.org.people is not available in this host",
        ))
    }
    /// `ctx.org.places()` — the org's locations. Same shape and same
    /// fail-closed default as `org_people`.
    async fn org_places(&self) -> Result<serde_json::Value, HostError> {
        Err(HostError::backend(
            "ctx.org.places is not available in this host",
        ))
    }
    /// `ctx.org.assignments()` — who holds which position where.
    async fn org_assignments(&self) -> Result<serde_json::Value, HostError> {
        Err(HostError::backend(
            "ctx.org.assignments is not available in this host",
        ))
    }
    /// `ctx.platform.facts(opts)` — ids and counts about every org, for an
    /// internal app. `opts` carries `weeks` only; the org is the host's. The
    /// same fail-closed default as `org_people`.
    async fn platform_facts(
        &self,
        _opts: serde_json::Value,
    ) -> Result<serde_json::Value, HostError> {
        Err(HostError::backend(
            "ctx.platform.facts is not available in this host",
        ))
    }
    /// `ctx.airway.run(pipelineRef, variables)` — seed an Airway ELT run.
    /// Returns `{ runId }`; the run is driven asynchronously by the worker
    /// fleet (it does not block on completion — ELT runs routinely exceed
    /// the function timeout ceiling).
    async fn airway_run(
        &self,
        pipeline_ref: String,
        variables: serde_json::Value,
    ) -> Result<serde_json::Value, HostError>;
    /// `ctx.warehouse.query(database, sql)` — a read against a named database.
    async fn warehouse_query(
        &self,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, HostError>;
    /// `ctx.warehouse.{insert,exec,upsert}` — write to one of the app's
    /// configured destination databases. `op` is `"insert"`, `"exec"`, or
    /// `"upsert"` — a registry sub-op, the broker having refused one off the
    /// list (`env_policy::HostOp`); `payload` carries
    /// `{ database, table?, rows?, sql? }` depending on `op`. Validated
    /// against the project's configured databases (§11.3) before execution.
    async fn warehouse_write(
        &self,
        op: String,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, HostError>;
    /// `ctx.secrets.set(key, value)` — upsert an app-scoped secret
    /// (`apps/<app_id>/<key>`) into the same namespace `ctx.env` reads. Gated
    /// by the fail-closed `secrets.write` manifest capability.
    async fn secrets_set(&self, key: String, value: String)
    -> Result<serde_json::Value, HostError>;
    /// `ctx.email.send(input)` — send email on behalf of the app. Gated by the
    /// fail-closed `email.send` manifest capability; the platform controls the
    /// `from` address (the author may set `replyTo` only). `input` is the JS
    /// payload object; returns `{ messageId }`.
    async fn send_email(&self, input: serde_json::Value) -> Result<serde_json::Value, HostError>;
    /// `ctx.notify(input)` — tell people in the org something, by user, role
    /// at a location, or team. Gated by the fail-closed `notify` capability;
    /// the platform resolves recipients and channels and never hands the
    /// function an address. Returns `{ notificationId, recipients, skipped }`.
    async fn notify(&self, input: serde_json::Value) -> Result<serde_json::Value, HostError>;
    /// `ctx.storage.{getUploadUrl,getDownloadUrl,put,get,head,list,delete,copy}`
    /// — presigned S3 file storage scoped to the app's silo. `op` selects the
    /// operation (a `storage.*` registry sub-op); `payload` carries its args.
    /// Gated by the fail-closed `storage.{read,write}` manifest capabilities,
    /// per op in the registry. Mirrors `warehouse_write`'s
    /// single-op-dispatcher shape.
    async fn storage(
        &self,
        op: String,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, HostError>;
    /// `ctx.tx(database, fn)` and `ctx.oltp.tx(fn)` — a multi-statement
    /// transaction on a pinned connection. `op` is one of `begin` / `begin_oltp`
    /// / `query` / `exec` / `commit` / `rollback`; `payload` carries its args. Same single-op-dispatcher shape
    /// as `warehouse_write`, and gated by the same fail-closed `destinations`
    /// allowlist — a transaction is a write, so it may not reach a database the
    /// function did not declare.
    ///
    /// The op split exists because the transaction has to stay open across
    /// `await`s **in the author's JavaScript**: the isolate holds a handle id
    /// and the pinned connection lives here.
    async fn tx(
        &self,
        op: String,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, HostError>;
    /// `ctx.oltp.{query,exec}` — read/write the app's OWN per-org OLTP schema
    /// (`app_<writer>`) on the managed Postgres tenant. `op` is `query` or
    /// `exec`; `payload` carries `{ sql, params? }`. Gated by the fail-closed
    /// `oltp` manifest capability, and scoped to the app's own writer role — so unlike `ctx.warehouse` (read-only analyst on a
    /// managed database) it can write, and cannot see another app's data.
    async fn oltp(
        &self,
        op: String,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, HostError>;
    /// `ctx.airhouse.{query,exec,append}` — the app's own facts in its
    /// workspace's Airhouse, in the schema `app_<writer>` derived from its slug.
    /// Written as the app whoever invoked; every statement is checked against
    /// that schema first. Gated by the fail-closed `airhouse` manifest capability.
    async fn airhouse(
        &self,
        op: String,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, HostError>;
}
