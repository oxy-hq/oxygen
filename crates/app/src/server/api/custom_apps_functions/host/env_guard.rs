//! Where each host op asks the environment policy, and how a held op holds.
//!
//! The decision is `env_policy::EnvPolicy::decide`, made once per op and never
//! re-derived here; this is only the mechanism. Production answers `Allow` to
//! every op, so on production each helper below returns what it was given and
//! records nothing. Every held or refused call is noted in the invocation's
//! held-write log (`held_log`), under its op and the identifiers it touched —
//! except in a production run reading a branch, which holds as staging does
//! but writes no `app.staging.held` row ([`ProjectFunctionHost::note_held`]).
//!
//! How each kind of op holds:
//!
//! - a pure write is not sent ([`ProjectFunctionHost::admit_op`]);
//! - `ctx.fetch` sends a read method with no body and answers anything else
//!   409, like `oxyc proxy`;
//! - `ctx.email.send` resolves `{ held: true }`;
//! - an OLTP statement is sent only when it is one read calling no
//!   side-effect function, on a read-only session inside a `READ ONLY`
//!   transaction — or, with the org's OLTP staging branch (P4b), runs there
//!   unless it reaches outside that database (`oltp_guard`);
//! - a held `tx.commit` rolls back.
//!
//! An op the policy isolates to a home is routed by the op itself
//! (`env_homes`); one that reaches [`ProjectFunctionHost::admit_op`] with
//! `Isolate` is refused rather than run on production. A statement on a
//! `ctx.tx` handle is decided by the home its `begin` was isolated to.

use std::sync::atomic::Ordering;

use super::super::env_policy::{self, Decision, HostOp, Target};
use super::*;

/// The identifiers a held call is logged under — never values, like every
/// write audit row: `(plane, namespace, verb, table)`.
pub(super) type HeldTarget<'a> = (&'static str, &'a str, &'a str, &'a str);

impl ProjectFunctionHost {
    /// `Ok` when `op` may run as asked. Otherwise the call is noted in the
    /// held-write log under `target` and the error it throws is returned.
    ///
    /// An op with an isolated home asks [`Self::route_op`] instead; one that
    /// reaches here with `Isolate` is refused rather than run against
    /// production.
    pub(super) async fn admit_op(&self, op: HostOp, target: HeldTarget<'_>) -> Result<(), String> {
        self.admit_decided(op, self.policy.decide(op), target).await
    }

    /// [`Self::admit_op`] for a decision the caller already asked for — one
    /// that depends on what the call names (`EnvPolicy::decide_on_database`,
    /// `decide_in_schema`). `Isolate` reaching here is a path that does not
    /// route to the home: refused, and logged, never run on production.
    pub(super) async fn admit_decided(
        &self,
        op: HostOp,
        decided: Decision,
        target: HeldTarget<'_>,
    ) -> Result<(), String> {
        match decided {
            Decision::Allow => Ok(()),
            Decision::Isolate(_) => {
                self.note_held(op, target).await;
                Err(env_policy::refused_message(
                    op,
                    self.policy.environment(),
                    env_policy::MISROUTED_FIX,
                ))
            }
            Decision::Hold => {
                self.note_held(op, target).await;
                Err(self.held_error(op))
            }
            Decision::Refuse { fix } => {
                self.note_held(op, target).await;
                Err(match self.policy.branch_reason() {
                    Some(reason) => env_policy::branch_refused_message(op, &reason, fix),
                    None => env_policy::refused_message(op, self.policy.environment(), fix),
                })
            }
        }
    }

    /// What a held `op` throws: the environment's hold, or a preview's when a
    /// production run reads a branch.
    pub(super) fn held_error(&self, op: HostOp) -> String {
        match self.policy.branch_reason() {
            Some(reason) => env_policy::branch_held_message(op, &reason),
            None => env_policy::held_message(op, self.policy.environment()),
        }
    }

    /// [`Self::admit_op`] for a run found reading a branch; `reason` says how
    /// (`staged_invocation::staged_reason`). The policy decides
    /// (`EnvPolicy::decide_on_branch`); a refusal here is a classified
    /// refusal, never a failure that pages.
    pub(super) async fn admit_on_branch(
        &self,
        op: HostOp,
        target: HeldTarget<'_>,
        reason: &str,
    ) -> Result<(), String> {
        let fix = match self.policy.decide_on_branch(op) {
            Decision::Allow => return Ok(()),
            // An isolated op routes through `route_op`; asked here, it is
            // refused rather than run against production.
            Decision::Isolate(_) => Some(env_policy::MISROUTED_FIX),
            Decision::Hold => None,
            Decision::Refuse { fix } => Some(fix),
        };
        self.on_branch.store(true, Ordering::SeqCst);
        self.note_held(op, target).await;
        Err(match fix {
            Some(fix) => env_policy::branch_refused_message(op, reason, fix),
            None => env_policy::branch_held_message(op, reason),
        })
    }

    /// Whether `op` is held here — for the ops that hold by a mechanism of
    /// their own rather than by not running.
    pub(super) fn holds(&self, op: HostOp) -> bool {
        holds(self.policy.decide(op))
    }

    /// [`Self::holds`] for a statement, commit or rollback on a `ctx.tx`
    /// handle opened into `opened_into` (`EnvPolicy::decide_on_handle`): on a
    /// handle whose `begin` was isolated, nothing is held — its connection is
    /// the isolated home.
    pub(super) fn holds_on_handle(&self, op: HostOp, opened_into: Option<Target>) -> bool {
        holds(self.policy.decide_on_handle(op, opened_into))
    }

    /// Whether a `HeldInStaging` / `EnvironmentRefused` label on an error from
    /// this host is the policy's own: outside production, or in a production
    /// run found reading a branch (a pin in scope, or a refusal for one).
    /// Anywhere else the label is honoured as nothing, so a production failure
    /// always pages.
    pub(super) fn policy_decides(&self) -> bool {
        !self.policy.is_production()
            || self.policy.branch_reason().is_some()
            || self.on_branch.load(Ordering::SeqCst)
    }

    /// Log one held call for this invocation's `app.staging.held` row.
    ///
    /// Not for a production-admitted run reading a branch: the row's
    /// `environment` column would say `production` for a preview's calls, and
    /// production's activity is read from that column. Its held and refused
    /// calls still throw, classified, and are traced here.
    pub(super) async fn note_held(&self, op: HostOp, target: HeldTarget<'_>) {
        self.note_held_with(op, target, None).await;
    }

    /// [`Self::note_held`], with the note a held entry carries (`oltp_guard`).
    pub(super) async fn note_held_with(
        &self,
        op: HostOp,
        (plane, namespace, verb, table): HeldTarget<'_>,
        note: Option<&'static str>,
    ) {
        tracing::info!(
            op = op.name(),
            environment = %self.policy.environment(),
            reads_branch = self.policy.is_production(),
            "host call held"
        );
        if self.policy.is_production() {
            return;
        }
        let record = WriteRecord {
            plane,
            namespace: namespace.to_string(),
            verb: verb.to_string(),
            table: table.to_string(),
            rows: None,
            statements: 1,
            op: Some(op.name()),
            note,
        };
        let (trace_id, _) = Self::trace_context();
        self.held.note(record, trace_id).await;
    }

    /// A `ctx.fetch` outside production is sent only when it is a read method
    /// carrying no body; anything else answers 409, like `oxyc proxy`.
    /// `None`: send it.
    pub(super) async fn held_fetch(
        &self,
        method: &str,
        host: Option<&str>,
        has_body: bool,
    ) -> Option<serde_json::Value> {
        let read = env_policy::is_read_method(method) && !has_body;
        if read || !self.holds(HostOp::Fetch) {
            return None;
        }
        self.note_held(
            HostOp::Fetch,
            ("fetch", host.unwrap_or_default(), method, ""),
        )
        .await;
        Some(env_policy::held_fetch_response(
            method,
            host,
            &self.held_error(HostOp::Fetch),
        ))
    }

    /// A `ctx.email.send` outside production resolves held. `None`: send it.
    pub(super) async fn held_email(&self, input: &serde_json::Value) -> Option<serde_json::Value> {
        if !self.holds(HostOp::EmailSend) {
            return None;
        }
        self.note_held(HostOp::EmailSend, ("email", "", "SEND", ""))
            .await;
        Some(env_policy::held_email_result(
            input,
            &self.held_error(HostOp::EmailSend),
        ))
    }
}

/// Whether `decided` holds an op that holds by a mechanism of its own. An
/// isolated op is not held: it runs, on its home.
pub(super) fn holds(decided: Decision) -> bool {
    match decided {
        Decision::Allow | Decision::Isolate(_) => false,
        Decision::Hold | Decision::Refuse { .. } => true,
    }
}
