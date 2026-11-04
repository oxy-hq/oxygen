//! The values that cross into and out of a function run: the `ctx` the
//! isolate is handed, the request and response, the row caps and the
//! error the route handler maps onto an SSE `error` frame.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Per-invocation context handed to the isolate. Built fresh for every
/// invocation from the resolved identity (design doc §11.7) — never cached
/// across invocations.
#[derive(Debug, Clone, Serialize)]
pub struct InvocationCtx {
    pub user: CtxUser,
    pub env: BTreeMap<String, String>,
    /// `ctx.airhouse.schema` — the app's own Airhouse schema, present only when
    /// the function declared the `airhouse` capability.
    #[serde(rename = "airhouseSchema", skip_serializing_if = "Option::is_none")]
    pub airhouse_schema: Option<String>,
    /// `ctx.channel` (and its alias `ctx.environment`, the environments
    /// design's name): the app environment this invocation runs in —
    /// `"production"`, or `"staging"`, where the host holds every write
    /// (`custom_apps_functions::env_policy`).
    pub channel: String,
}

/// One org team the caller belongs to, as surfaced through `ctx.user.teams`.
///
/// **Scoped to the app's own org.** A team the caller holds in some *other* org
/// is never reported here — the same user in two tenants must not learn one
/// tenant's team names from the other's app.
#[derive(Debug, Clone, Serialize)]
pub struct CtxTeam {
    pub id: String,
    pub name: String,
}

/// Where this invocation's identity came from.
///
/// The distinction is **"is there a caller to attribute this to"**, not "did a
/// human cause it". Every background path runs under the org **owner's**
/// `user_id` (the invocation row needs a non-null FK, and `ctx.secrets` needs a
/// `created_by`) and carries no caller — including an operator's manual
/// **Run now**, which `trigger_function_job` deliberately routes down the same
/// system path under the owner identity, with no caller context beyond whatever
/// `input` the trigger was given. So a person may well have clicked; the platform
/// simply did not carry who through the task queue.
///
/// A function that branches on identity — "email the person who clicked", "show
/// the admin view" — has to be able to tell the two apart, and an email-sniffing
/// check (`endsWith("@system.oxy")`) is exactly the kind of heuristic that
/// silently stops working.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CtxIdentityKind {
    /// A signed-in human called the function over its HTTP route.
    User,
    /// No caller to attribute it to: a schedule tick, an Airway transform step,
    /// or an operator's manual **Run now** (which the job trigger routes down
    /// this same path). Every *caller* field (`name`, `picture`, `orgRole`,
    /// `teams`) is absent, and `email` is the synthetic
    /// `schedule+<fn>@system.oxy`.
    ///
    /// A manual run therefore cannot reach the operator who triggered it —
    /// `run_function_job` discards the authenticated user, and the task payload
    /// has nowhere to put it. Threading it through is a real follow-up, and a
    /// behaviour change: the run would then execute under that operator's
    /// authority rather than the owner's.
    System,
}

/// The identity of whoever (or whatever) invoked this function.
///
/// Assembled server-side from the authenticated session on every invocation and
/// never cached across them, so **nothing here is client-supplied** — that is
/// the whole point of reading identity from `ctx` rather than from the request
/// body. See `internal-docs/custom-apps-user-identity.md` for the full contract
/// and for what the *client* side (`useShellContext`) can and cannot be trusted
/// for.
#[derive(Debug, Clone, Serialize)]
pub struct CtxUser {
    /// `users.id`. On a system invocation this is the org owner's id, not a
    /// caller — check `kind` before attributing anything to it.
    pub id: String,
    /// `users.email`, or the synthetic `schedule+<fn>@system.oxy` when
    /// `kind == "system"`.
    ///
    /// `None` — serialized as `ctx.user.email === null` — for a frontline
    /// worker enrolled without a mailbox. Deliberately NOT flattened to `""`:
    /// a function branching on "can I email this person" must be able to tell
    /// "no address" from "an address that happens to be empty", and an app that
    /// passes `""` to `ctx.email.send` gets an SES rejection instead of an
    /// obvious `null` check. See `internal-docs/frontline-identity.md`.
    pub email: Option<String>,
    /// The org that owns this app — the tenant boundary for any query the
    /// function runs.
    ///
    /// Serialized as `orgId`. Before 2026-08-21 this went out as `org_id`,
    /// which meant the documented `ctx.user.orgId` was `undefined` — a silent
    /// footgun for any SQL filtering on it. `__buildCtx` still mirrors the old
    /// `org_id` key so functions written against the shipped behaviour keep
    /// working.
    #[serde(rename = "orgId")]
    pub org_id: String,
    /// `users.name` — display identity, absent on a system invocation.
    ///
    /// Free text the user controls. Fine for a greeting or an audit row; never
    /// a key, and never interpolated into SQL or HTML without escaping.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `users.picture` — an avatar URL, absent when unset or on a system
    /// invocation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
    /// The caller's role **within this app** — `"admin"`, `"member"`, or absent.
    /// Server-derived: `"admin"` is `Ring::AppAdmin` in `oxy-authz` (an app grant,
    /// org owner/admin, or staff break-glass), and `"member"` is any grant the
    /// caller holds on the app — **direct (`app_members`) or through a team they
    /// belong to (`app_team_grants` × `org_team_members`)**. An app gates its
    /// privileged surface on this rather than on a client-side flag or a
    /// hard-coded email allowlist.
    ///
    /// A **system** invocation runs under the org owner, so this reads `"admin"`
    /// there — a schedule has owner authority by construction. Gate on `kind`
    /// too if a surface must be human-only.
    #[serde(rename = "appRole", skip_serializing_if = "Option::is_none")]
    pub app_role: Option<String>,
    /// The caller's role in the owning **org** — `"owner"`, `"admin"`, or
    /// `"member"`; absent when they reach the app without an org membership
    /// (Oxy staff on break-glass) or on a system invocation.
    ///
    /// A *fact* read straight off `org_members.role`, not an authorization
    /// verdict: org standing and app standing are different rings, and an app
    /// admin need not be an org Admin. Gate on [`Self::app_role`]; use this to
    /// explain, label, or route — "your org admin can change this".
    #[serde(rename = "orgRole", skip_serializing_if = "Option::is_none")]
    pub org_role: Option<String>,
    /// The org teams the caller belongs to, name-sorted, scoped to this app's
    /// org. Empty when they belong to none, and always present so a function can
    /// `.some(...)` without a null check.
    ///
    /// Teams are how an org grants an app to a group it already recognises, so
    /// they are useful for *shaping* a view (default the Finance team to the
    /// finance tab). They are not a permission: a team only means something on
    /// an app through `app_team_grants`, which is already folded into
    /// [`Self::app_role`].
    pub teams: Vec<CtxTeam>,
    /// Whether a human or the platform invoked this function.
    pub kind: CtxIdentityKind,
    /// Where the caller may act — the operating graph's answer, decided from
    /// their assignments before the function runs and applied by the function
    /// through `@oxy-hq/sdk/ops`. A system invocation reaches everywhere; a
    /// lookup failure reaches nowhere, so a blip cannot widen anything.
    /// `internal-docs/operating-graph.md` §3.3.
    pub reach: oxy_app_core::operating_graph::reach::Reach,
}

/// Result of running a function to completion.
#[derive(Debug, Deserialize)]
pub struct FnResponse {
    #[serde(default = "default_status")]
    pub status: u16,
    #[serde(default)]
    pub body: String,
}

fn default_status() -> u16 {
    200
}

/// Errors surfaced to the route handler as SSE `event: error` frames.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("function threw: {0}")]
    Js(String),
    #[error("function was cancelled")]
    Cancelled,
    #[error("function execution timed out")]
    Timeout,
    /// The isolate breached its heap ceiling and was terminated.
    ///
    /// Before the ceiling existed this case did not surface as an error at all:
    /// V8's own default limit is far above the pod's cgroup limit, so the
    /// kernel OOM killer reached the process first and took down every other
    /// app it was serving. This variant is that failure, made survivable and
    /// attributable to one invocation.
    #[error("function exceeded its memory limit")]
    OutOfMemory,
    /// No concurrency permit came free within the queue budget.
    ///
    /// The platform declining to start the work, not the function failing. The
    /// caller maps it to the `shed` invocation status, which
    /// [`super::super::failure_signal::Failure::of`] treats like a cancellation: no
    /// page, no `error.type`, and no dent in the app's availability. The count
    /// lives in `oxy_custom_app_admission_shed_total`.
    ///
    /// **Not a 503.** The route path answers over SSE
    /// (`mod.rs::sse_response`), so every outcome — success or failure — is a
    /// 200 carrying an event. A shed surfaces as `{"error": "shed", …}` in
    /// that stream; giving it a real status code would mean changing the
    /// response contract for every invocation, not just this one.
    #[error("too many functions are running; try again shortly")]
    Overloaded,
    #[error("internal runtime error: {0}")]
    Internal(String),
}

/// Function-scoped row cap for `ctx.query` (design doc §11.5): 10x the UI
/// `MAX_ROWS` cap, since ETL functions legitimately need more rows than a
/// rendered table. Genuinely large scans use `ctx.queryStream` instead.
pub const FUNCTION_MAX_ROWS: usize = 100_000;

/// Row cap for `ctx.queryStream` (design doc §11.5): a higher ceiling than
/// `FUNCTION_MAX_ROWS` for genuinely large scans. The MVP implementation
/// fetches up to this many rows in one shot and yields them to the isolate
/// in client-side batches; a true warehouse-cursor implementation is future
/// work.
pub const FUNCTION_STREAM_MAX_ROWS: usize = 1_000_000;

/// The triggering HTTP request, as the function's `req` argument sees it.
///
/// Bundled rather than passed as three positional parameters: `run` is already
/// at the arity `internal-docs/backend-architecture.md` caps out at.
///
/// `headers` has already been filtered by the caller — the runtime does not
/// re-check it, so anything placed here reaches app code verbatim.
pub struct FnRequest {
    pub method: String,
    pub headers: std::collections::BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl FnRequest {
    /// A request carrying only a body — no real HTTP request behind it. This is
    /// what the schedule / Airway / manual-run paths synthesise, and what a test
    /// exercising handler behaviour rather than request plumbing wants.
    pub fn from_body(body: Vec<u8>) -> Self {
        Self {
            method: "POST".to_string(),
            headers: std::collections::BTreeMap::new(),
            body,
        }
    }
}
