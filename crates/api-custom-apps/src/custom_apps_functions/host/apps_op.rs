//! `ctx.apps.call` on the host (app events design §8): another app's function
//! run now, in this app's environment, as that app, its JSON answer returned.
//!
//! **Checks, in order, each before anything runs:** the org flag `app_events`;
//! `app` and `function` are non-empty strings; the pair is one the manifest's
//! top-level `calls` declares (nothing the function sends adds to it); the
//! body is JSON under [`MAX_CALL_BODY_BYTES`]; `timeoutMs` is a whole number of
//! milliseconds; the chain is not past [`MAX_DEPTH`]. The target app is then
//! found among this org's apps by slug — never another org's.
//!
//! **The callee** runs the build its environment serves (production calls the
//! target's production build, staging its staging build, a sandbox the target's
//! same sandbox; there is no fallback to production), with its own secrets,
//! capabilities, OLTP schema and environment policy. **Who it runs as** is the
//! chain's person when there is one (`ctx.user` is that person, their
//! credential's ceiling included, with their role, teams and reach resolved
//! again for the callee's app), after `Action::AppCall` asks whether they may
//! open the target app ([`ProjectFunctionHost::authorize_target`]); a chain with
//! no person (a schedule, an event, a send delivered as the system) runs it as
//! the system. A send delivered as its sender (`host::apps_send`) is such a
//! chain's person too. The callee sees `ctx.caller` for attribution. It is admitted only if it is
//! marked `"callable": true` (`queued_admission`).
//!
//! **Budget and depth.** The callee's timeout is the smallest of `timeoutMs`,
//! its own function's timeout and what the caller has left less a margin
//! ([`callee_budget`]). A call and a published event share one depth counter
//! and [`MAX_DEPTH`].
//!
//! **Capacity.** A callee runs on its root's admission and takes no org or
//! global permit: its caller takes it a nested permit and a slot in the chain
//! (`limits`). Only a root caller waits for either, up to its budget; a callee
//! that calls on is refused at once (`AppCallBusy`), so no run waits for
//! capacity that only a run like it can free and a chain cannot park the pool.
//!
//! **The answer** is the callee's response body parsed as JSON (empty is
//! `null`), under [`MAX_CALL_BODY_BYTES`]. Anything else is an error whose text
//! starts with an `AppCall…` label ([`CallErrorKind`]); a callee's thrown
//! message passes through, truncated.

use std::time::{Duration, Instant};

use oxy_custom_apps::calls::{CallErrorKind, MAX_CALL_BODY_BYTES, cycle_refusal, truncate_error};
use oxy_custom_apps::events::MAX_DEPTH;

use super::super::limits::{self, ChainBudget, ChainRefusal, NestedRefusal};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::super::call_run::{self, CallRun, ChainStart};
use super::super::host_error::HostError;
use super::super::runtime::{CtxCaller, EventCause};
use super::*;
use entity::apps;

/// The whole of what a function without `calls` is told.
pub(in crate::custom_apps_functions) const CAPABILITY_MISSING: &str = "AppsCapabilityMissing: this function has not declared the apps it calls (add \"calls\": { \"<app>\": [\"<function>\"] } to oxy-app.json)";
/// `app` or `function` that is not a non-empty string.
pub(super) const NAMES_INVALID: &str =
    "AppCallRefused: `app` and `function` must be non-empty strings";
/// What a call is refused with while the org's `app_events` flag is off.
pub(super) const NOT_ENABLED: &str =
    "AppCallRefused: app calls are not enabled for this org (the `app_events` flag)";
/// A `timeoutMs` that is not a whole number of milliseconds from 1.
const TIMEOUT_INVALID: &str = "AppCallRefused: `timeoutMs` must be a whole number from 1";

/// The pair not under `calls`.
pub(super) fn undeclared(app: &str, function: &str) -> String {
    format!(
        "AppCallRefused: oxy-app.json does not declare a call to {app}/{function} (add it under \"calls\")"
    )
}

/// The person a run is for: what a call from it carries into the callee,
/// which runs as them (their credential's ceiling included) and is asked
/// whether they may open the target app (`Action::AppCall`).
#[derive(Clone, Debug)]
pub struct Person {
    pub caller: oxy_server_authz::Caller,
    pub user_id: Uuid,
    pub email: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
    /// Stood up at a send's delivery from the login session the send was made
    /// on (`app_function_executor::sender`). That session's end is not known
    /// (only that it was no later than 30 days from the send), so this person's
    /// own sends carry no invoker and run as the system
    /// (`host::apps_send::send_invoker`): a chain of sends never carries a
    /// login session past the hop it was checked at. A token is re-admitted at
    /// every hop and needs no such stop.
    pub relayed_session: bool,
}

/// How an invocation sits in a chain of calls and events, and what it may
/// spend. Built once per run ([`ChainScope::for_run`]); the default is a run
/// outside any chain with no budget (a test double).
#[derive(Clone, Debug)]
pub struct ChainScope {
    /// The run's depth: its event's, or the callee's. `None` outside a chain.
    position: Option<i32>,
    root: Option<Uuid>,
    actor: Option<String>,
    deadline: Option<Instant>,
    /// Ended when the run ends, which ends every call it is still waiting on.
    cancel: CancellationToken,
    /// The chain of calls this run is in: shared with its callees.
    chain: std::sync::Arc<ChainBudget>,
    /// Whether this run is itself a callee (it holds a nested permit).
    callee: bool,
    /// The (app, function) pairs of the synchronous chain above this run,
    /// its caller last. Empty for a root run.
    path: Vec<(String, String)>,
}

impl Default for ChainScope {
    fn default() -> Self {
        Self {
            position: None,
            root: None,
            actor: None,
            deadline: None,
            cancel: CancellationToken::new(),
            chain: ChainBudget::new(),
            callee: false,
            path: Vec::new(),
        }
    }
}

impl ChainScope {
    /// The scope of a run that has `timeout` from now and ends when `cancel`
    /// does.
    pub fn for_run(
        event: Option<&EventCause>,
        chain: Option<&ChainStart>,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Self {
        Self {
            position: event.map(|e| e.depth).or(chain.map(|c| c.position)),
            root: chain.map(|c| c.root),
            actor: chain.and_then(|c| c.actor.clone()),
            deadline: Some(Instant::now() + timeout),
            cancel: cancel.child_token(),
            chain: chain.map_or_else(ChainBudget::new, |c| c.budget.clone()),
            callee: chain.is_some(),
            path: chain.map_or_else(Vec::new, |c| c.path.clone()),
        }
    }

    /// The run is over: stop whatever it still has waiting.
    pub fn end(&self) {
        self.cancel.cancel();
    }

    /// What is left of the run's budget.
    fn remaining(&self) -> Duration {
        self.deadline.map_or(Duration::from_secs(300), |d| {
            d.saturating_duration_since(Instant::now())
        })
    }
}

/// The depth something made by a run at `position` has: 0 outside a chain.
pub(super) fn next_depth(position: Option<i32>) -> i32 {
    position.map_or(0, |p| p.saturating_add(1))
}

/// Where this run came from, for what it publishes or calls.
pub(super) struct Chain {
    pub position: Option<i32>,
    pub root: Uuid,
    pub actor: Option<String>,
    /// The event whose delivery started the run, when one did.
    pub causation: Option<Uuid>,
}

/// A call that passed every check on its arguments.
pub(super) struct Target {
    pub app: String,
    pub function: String,
    pub body: Value,
}

impl ProjectFunctionHost {
    /// This run's place in its chain.
    pub(super) fn chain(&self) -> Chain {
        if let Some(cause) = &self.event_cause {
            return Chain {
                position: Some(cause.depth),
                root: cause.root_id,
                actor: cause.actor.clone(),
                causation: Some(cause.id),
            };
        }
        let scope = &self.chain_scope;
        Chain {
            position: scope.position,
            root: scope.root.unwrap_or(self.identity.invocation_id),
            actor: match scope.position {
                Some(_) => scope.actor.clone(),
                None => self.identity.user_id.map(|user| user.to_string()),
            },
            causation: None,
        }
    }

    /// The synchronous chain down to and including this run.
    fn call_path(&self) -> Vec<(String, String)> {
        let mut path = self.chain_scope.path.clone();
        path.push((
            self.identity.app_slug.clone(),
            self.identity.function_name.clone(),
        ));
        path
    }

    /// `ctx.apps.call` and `ctx.apps.send`; see the module docs.
    pub(in crate::custom_apps_functions) async fn apps_op(
        &self,
        op: &str,
        payload: Value,
    ) -> Result<Value, HostError> {
        let host_op = HostOp::sub_op("apps", op)
            .ok_or_else(|| HostError::backend(format!("unknown op '{op}'")))?;
        let target = self.check_target(&payload).map_err(HostError::backend)?;
        match host_op {
            HostOp::AppsCall => self.call(target, &payload).await,
            HostOp::AppsSend => self
                .send(host_op, target, &payload)
                .await
                .map_err(HostError::from),
            _ => Err(HostError::backend(format!("unknown op '{op}'"))),
        }
    }

    /// The checks of the module docs that every op shares.
    pub(super) fn check_target(&self, payload: &Value) -> Result<Target, String> {
        use oxy_app::surface::feature_flags::{APP_EVENTS, FlagContext, is_enabled_for};
        if !is_enabled_for(APP_EVENTS, &FlagContext::org(self.org_id)) {
            return Err(NOT_ENABLED.to_string());
        }
        let name = |key: &str| {
            payload
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
        };
        let (Some(app), Some(function)) = (name("app"), name("function")) else {
            return Err(NAMES_INVALID.to_string());
        };
        if !self.caps.calls.allows(app, function) {
            return Err(undeclared(app, function));
        }
        let body = payload.get("body").cloned().unwrap_or(Value::Null);
        let bytes = serde_json::to_vec(&body).map_or(usize::MAX, |b| b.len());
        if bytes > MAX_CALL_BODY_BYTES {
            return Err(CallErrorKind::Refused.message(format!(
                "the body is {bytes} bytes; the limit is {MAX_CALL_BODY_BYTES}"
            )));
        }
        let depth = next_depth(self.chain().position);
        if depth > MAX_DEPTH {
            return Err(CallErrorKind::DepthExceeded.message(format!(
                "this would be depth {depth}; the limit is {MAX_DEPTH}"
            )));
        }
        Ok(Target {
            app: app.to_string(),
            function: function.to_string(),
            body,
        })
    }

    /// The target app, among this org's apps.
    pub(super) async fn find_target_app(&self, slug: &str) -> Result<entity::apps::Model, String> {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
        entity::apps::Entity::find()
            .filter(entity::apps::Column::OrgId.eq(self.org_id))
            .filter(entity::apps::Column::Slug.eq(slug))
            .one(&self.db)
            .await
            .map_err(|e| {
                CallErrorKind::Failed.message(format!("could not look up the app `{slug}`: {e}"))
            })?
            .ok_or_else(|| CallErrorKind::NotFound.message(format!("this org has no app `{slug}`")))
    }

    /// May the person this run is for open `app` in this run's environment?
    /// The one question a call asks, at the target's door ([`app_call_access::may_call_as`](super::app_call_access::may_call_as)).
    /// A run with no person (a schedule, an event, a send delivered as the
    /// system) is the system and is not asked; it carries no more than a
    /// background run has.
    pub(super) async fn authorize_target(&self, app: &apps::Model) -> Result<(), String> {
        let Some(person) = &self.person else {
            return Ok(());
        };
        let environment = self.policy.environment();
        if super::app_call_access::may_call_as(
            &self.db,
            &person.caller,
            app,
            environment,
            "custom_app_call",
        )
        .await
        {
            Ok(())
        } else {
            Err(CallErrorKind::Refused.message(format!(
                "the person this call runs for may not open `{}` in {environment}",
                app.slug
            )))
        }
    }

    /// The capacity one callee of this run needs: a slot in the chain and a
    /// nested permit, both held until the callee ends. A root caller waits for
    /// them up to what it has left; a callee that calls on never waits.
    async fn callee_capacity(&self) -> Result<(limits::ChainSlot, limits::NestedPermit), String> {
        let busy = |text: &str| CallErrorKind::Busy.message(text);
        let scope = &self.chain_scope;
        let wait = scope
            .remaining()
            .saturating_sub(oxy_custom_apps::calls::BUDGET_MARGIN);
        let give_up = Instant::now() + if scope.callee { Duration::ZERO } else { wait };
        let slot = loop {
            match scope.chain.try_slot() {
                Ok(slot) => break slot,
                Err(ChainRefusal::Total) => {
                    return Err(busy("this chain of calls has made its 32 calls"));
                }
                Err(ChainRefusal::InFlight) if Instant::now() >= give_up => {
                    return Err(busy(
                        "this chain of calls has its callees in flight already",
                    ));
                }
                Err(ChainRefusal::InFlight) => {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        };
        let left = give_up.saturating_duration_since(Instant::now());
        let permit = if scope.callee {
            limits::try_nested(self.org_id)
        } else {
            limits::wait_nested(self.org_id, left).await
        };
        let permit = permit.map_err(|why| {
            busy(match why {
                NestedRefusal::Org => "this org has all its nested function isolates running",
                NestedRefusal::Global => "this server has all its nested function isolates running",
            })
        })?;
        Ok((slot, permit))
    }

    /// `ctx.apps.call`: run the callee now and answer with its JSON.
    async fn call(&self, target: Target, payload: &Value) -> Result<Value, HostError> {
        let asked = timeout_of(payload).map_err(HostError::backend)?;
        let path = self.call_path();
        if let Some(refusal) = cycle_refusal(&path, (&target.app, &target.function)) {
            return Err(HostError::backend(refusal));
        }
        let app = self
            .find_target_app(&target.app)
            .await
            .map_err(HostError::backend)?;
        self.authorize_target(&app)
            .await
            .map_err(HostError::backend)?;
        let (_slot, _permit) = self.callee_capacity().await.map_err(HostError::backend)?;
        let chain = self.chain();
        let input = serde_json::to_vec(&target.body)
            .map_err(|e| HostError::backend(CallErrorKind::Refused.message(e.to_string())))?;
        let run = CallRun {
            caller: CtxCaller {
                app: self.identity.app_slug.clone(),
                function: self.identity.function_name.clone(),
                invocation_id: self.identity.invocation_id,
                actor: chain.actor.clone(),
            },
            position: next_depth(chain.position),
            root: chain.root,
            asked,
            remaining: self.chain_scope.remaining(),
            parent: self.identity.invocation_id,
            person: self.person.clone(),
            chain_budget: self.chain_scope.chain.clone(),
            path,
            budget: Duration::ZERO,
        };
        // Ends the callee when this call is dropped (the caller's isolate was
        // stopped) as well as when the caller's run ends.
        let cancel = self.chain_scope.cancel.child_token();
        let _stop_callee = cancel.clone().drop_guard();
        let reported = call_run::run_call(
            &self.db,
            &app,
            self.policy.environment(),
            &target.function,
            run,
            input,
            cancel,
            self.query_exec.clone(),
            self.preagg.clone(),
        )
        .await
        .map_err(|f| HostError::backend(f.kind.message(f.message)))?;
        answer_of(reported).map_err(HostError::backend)
    }
}

/// `timeoutMs`: absent, or a whole number of milliseconds from 1.
fn timeout_of(payload: &Value) -> Result<Option<Duration>, String> {
    match payload.get("timeoutMs") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|ms| *ms >= 1)
            .map(|ms| Some(Duration::from_millis(ms)))
            .ok_or_else(|| TIMEOUT_INVALID.to_string()),
    }
}

/// The caller's answer from how the callee's run ended.
fn answer_of(report: super::super::QueuedReport) -> Result<Value, String> {
    let label = |kind: CallErrorKind, text: &str| kind.message(truncate_error(text));
    match report.status {
        "success" if (200..300).contains(&report.http_status) => parse_answer(&report.body),
        "success" => Err(label(
            CallErrorKind::Failed,
            &format!(
                "the callee answered HTTP {}: {}",
                report.http_status,
                truncate_error(&report.body)
            ),
        )),
        "timeout" => Err(label(
            CallErrorKind::Timeout,
            "the callee did not answer in time",
        )),
        "shed" => Err(label(
            CallErrorKind::Busy,
            "no isolate came free for the callee; try again shortly",
        )),
        _ => Err(label(
            CallErrorKind::Failed,
            report.error.as_deref().unwrap_or("the callee failed"),
        )),
    }
}

/// The callee's response body as JSON: empty is `null`; too large or not JSON
/// is a failure.
fn parse_answer(body: &str) -> Result<Value, String> {
    if body.len() > MAX_CALL_BODY_BYTES {
        return Err(CallErrorKind::Failed.message(format!(
            "the callee's answer is {} bytes; the limit is {MAX_CALL_BODY_BYTES}",
            body.len()
        )));
    }
    if body.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(body)
        .map_err(|_| CallErrorKind::Failed.message("the callee's answer is not JSON"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(
        status: &'static str,
        http_status: u16,
        body: &str,
    ) -> super::super::super::QueuedReport {
        super::super::super::QueuedReport {
            invocation_id: Uuid::nil(),
            status,
            http_status,
            body: body.to_string(),
            error: Some("boom".into()),
            memory_limit: false,
        }
    }

    #[test]
    fn answers_map_to_their_kinds() {
        let kind = |r| CallErrorKind::of_message(&answer_of(r).unwrap_err());
        assert_eq!(
            answer_of(report("success", 200, "{\"a\":1}")),
            Ok(serde_json::json!({"a":1}))
        );
        assert_eq!(answer_of(report("success", 200, "")), Ok(Value::Null));
        assert_eq!(
            kind(report("success", 200, "nope")),
            Some(CallErrorKind::Failed)
        );
        assert_eq!(
            kind(report("success", 500, "x")),
            Some(CallErrorKind::Failed)
        );
        assert_eq!(kind(report("timeout", 0, "")), Some(CallErrorKind::Timeout));
        assert_eq!(kind(report("shed", 0, "")), Some(CallErrorKind::Busy));
        let thrown = answer_of(report("error", 0, "")).unwrap_err();
        assert_eq!(thrown, "AppCallFailed: boom");
    }

    #[test]
    fn a_callees_message_is_cut() {
        let mut r = report("error", 0, "");
        r.error = Some("x".repeat(900));
        assert!(answer_of(r).unwrap_err().chars().count() < 540);
    }

    #[test]
    fn timeout_must_be_a_whole_positive_number() {
        let of = |v: Value| timeout_of(&serde_json::json!({ "timeoutMs": v }));
        assert_eq!(
            of(serde_json::json!(1500)),
            Ok(Some(Duration::from_millis(1500)))
        );
        assert!(of(serde_json::json!(0)).is_err());
        assert!(of(serde_json::json!(1.5)).is_err());
        assert!(of(serde_json::json!("5")).is_err());
        assert_eq!(timeout_of(&serde_json::json!({})), Ok(None));
    }

    #[test]
    fn depth_counts_from_the_runs_position() {
        assert_eq!(next_depth(None), 0);
        assert_eq!(next_depth(Some(0)), 1);
        assert_eq!(next_depth(Some(MAX_DEPTH)), MAX_DEPTH + 1);
    }
}
