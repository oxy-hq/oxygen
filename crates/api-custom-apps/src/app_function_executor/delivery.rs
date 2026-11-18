//! One attempt of an event's delivery (app events design §6.6), on the
//! `app_function` task the relay queued for it (trigger `event`). The attempt
//! is opened on the delivery before the listener runs and closed with how it
//! ended ([`deliveries`]). A delivery that is not pending is skipped; one whose
//! event's body is gone or can never be read (`DeliveryStart::Dead`), or whose
//! listener's live build no longer subscribes to the event, is killed without
//! running; one whose body could not be read this time (the org's store was
//! not reached) fails the task for the queue to retry. Otherwise the listener
//! runs with the envelope (its `data` read from the org's store) as its body
//! and `ctx.delivery` set, and:
//!
//! - a 2xx answer acks the delivery;
//! - a thrown `NonRetryableError` kills it, and the task completes, so the
//!   queue spends no retry on it;
//! - anything else (an error, another answer, a timeout, an isolate killed at
//!   its heap limit) fails the task, so the queue retries it, and leaves the
//!   delivery pending, until the last attempt fails: that kills it.
//!
//! **The listener's output never reaches Oxy's Postgres,** like the body it
//! was handed (data placement). A delivered run's answer is a summary,
//! `delivered (HTTP <status>)`, never what the listener returned; its
//! invocation row keeps no result and only a capped error (`run_queued`); a
//! non-2xx answer is recorded by its status alone; and every error text the
//! task carries is capped ([`cap_error`]).

use agentic_core::delegation::TaskOutcome;
use oxy_custom_apps::events::OxyEvent;
use sea_orm::DbErr;
use uuid::Uuid;

use super::{FunctionRun, JobArgs, run_function};
use crate::app_events::cap_error;
use crate::app_events::deliveries::{self, AttemptOutcome, DeliveryInfo, DeliveryStart};
use crate::app_events::subscriptions;

/// One attempt of the delivery `delivery_id`, as the module docs describe:
/// open it, check the listener still subscribes, run it, settle the attempt.
pub(super) async fn deliver(job: &JobArgs, delivery_id: Uuid) -> TaskOutcome {
    let delivery = Delivery {
        job,
        id: delivery_id,
    };
    let (envelope, info) = match delivery.start().await {
        Ok(started) => started,
        Err(answered) => return answered,
    };
    let subscribed = subscriptions::listener_subscribes(
        &job.db,
        job.app_id,
        &job.environment,
        &job.function_name,
        &envelope.event_type,
    )
    .await;
    let ran = match subscribed {
        Ok(false) => {
            let reason = format!("listener no longer subscribes to {}", envelope.event_type);
            return delivery.kill(Some(info.attempt), None, &reason).await;
        }
        Err(e) => Err(format!("could not read the listener's subscription: {e}")),
        Ok(true) => run_listener(job, &envelope, &info).await,
    };
    delivery.settle(&info, ran).await
}

/// The delivery one task answers for, and how this module answers the task.
struct Delivery<'a> {
    job: &'a JobArgs,
    id: Uuid,
}

impl Delivery<'_> {
    /// The task completed with `answer`.
    fn done(&self, answer: String) -> TaskOutcome {
        TaskOutcome::Done {
            answer,
            metadata: Some(serde_json::json!({
                "app_id": self.job.app_id,
                "function_name": self.job.function_name,
                "event_delivery_id": self.id,
            })),
        }
    }

    /// The task failed because `what` could not be done (`e`): the queue
    /// retries it.
    fn failed(&self, what: &str, e: &DbErr) -> TaskOutcome {
        TaskOutcome::Failed(cap_error(&format!(
            "event delivery {}: {what}: {e}",
            self.id
        )))
    }

    /// [`deliveries::begin`]: the event and the attempt to run, or the task's
    /// answer when there is nothing to run. A body that could not be read
    /// this time fails the task (`begin` closed the attempt).
    async fn start(&self) -> Result<(OxyEvent, DeliveryInfo), TaskOutcome> {
        match deliveries::begin(&self.job.db, self.id).await {
            Ok(DeliveryStart::Run { envelope, info }) => Ok((envelope, info)),
            Ok(DeliveryStart::Skip { reason }) => Err(self.done(format!("skipped: {reason}"))),
            Ok(DeliveryStart::Dead { reason, attempt }) => {
                Err(self.kill(attempt, None, &reason).await)
            }
            Ok(DeliveryStart::Retry { reason }) => Err(TaskOutcome::Failed(cap_error(&format!(
                "event delivery {}: {reason}",
                self.id
            )))),
            Err(e) => Err(self.failed("could not open an attempt", &e)),
        }
    }

    /// Kill the delivery with `reason`, closing `attempt` (when one was
    /// opened) as `non_retryable` with the run's `invocation`; the task
    /// completes, so the queue spends no retry on it.
    async fn kill(
        &self,
        attempt: Option<i32>,
        invocation: Option<Uuid>,
        reason: &str,
    ) -> TaskOutcome {
        let outcome = AttemptOutcome::NonRetryable;
        match deliveries::finish_dead(&self.job.db, self.id, attempt, invocation, outcome, reason)
            .await
        {
            Ok(()) => self.done(cap_error(&format!("dead: {reason}"))),
            Err(e) => self.failed("could not record it dead", &e),
        }
    }

    /// Close attempt `info` with how the run ended (`ran`), and answer the
    /// task: an ack completes it, a `NonRetryableError` kills the delivery
    /// and completes it, anything else fails it for the queue's retry and,
    /// on the last attempt, kills the delivery.
    async fn settle(&self, info: &DeliveryInfo, ran: Result<FunctionRun, String>) -> TaskOutcome {
        let (invocation, outcome, error, answer) = match ran {
            Ok(run) => {
                let (outcome, error) = attempt_of(&run.listener_run());
                let answer = delivered(run.http_status);
                (Some(run.invocation_id), outcome, error, answer)
            }
            // Refused before it ran (its environment stopped taking events,
            // say), or never reached: an error the queue retries.
            Err(error) => (None, AttemptOutcome::Error, Some(error), String::new()),
        };
        let error = error.unwrap_or_default();
        // Each finish is fenced on this run's own attempt: if the queue
        // reclaimed the task and a newer run has begun, nothing is recorded.
        let (db, id, attempt) = (&self.job.db, self.id, info.attempt);
        match outcome {
            // A summary, never the listener's output. At-least-once: a run
            // whose ack is lost fails, and runs again.
            AttemptOutcome::Ok => match deliveries::finish_ok(db, id, attempt, invocation).await {
                Ok(()) => self.done(answer),
                Err(e) => self.failed("the listener answered, but the ack was not recorded", &e),
            },
            AttemptOutcome::NonRetryable => self.kill(Some(attempt), invocation, &error).await,
            AttemptOutcome::Error | AttemptOutcome::Timeout | AttemptOutcome::Crash => {
                let recorded = if info.is_last() {
                    deliveries::finish_dead(db, id, Some(attempt), invocation, outcome, &error)
                        .await
                } else {
                    deliveries::finish_err(db, id, attempt, invocation, outcome, &error).await
                };
                if let Err(e) = recorded {
                    tracing::warn!(delivery_id = %id, error = %e, "app events: an attempt's failure was not recorded");
                }
                TaskOutcome::Failed(cap_error(&error))
            }
        }
    }
}

/// A delivered run's answer: its status, never what the listener returned.
fn delivered(http_status: u16) -> String {
    format!("delivered (HTTP {http_status})")
}

/// Run the listener with `envelope` as its body, chaining what it publishes
/// from the event, `ctx.delivery` set to `info`.
#[cfg(feature = "custom-app-functions")]
async fn run_listener(
    job: &JobArgs,
    envelope: &oxy_custom_apps::events::OxyEvent,
    info: &deliveries::DeliveryInfo,
) -> Result<FunctionRun, String> {
    let body = serde_json::to_vec(envelope)
        .map_err(|e| format!("the envelope does not serialise: {e}"))?;
    let event = crate::custom_apps_functions::EventRun {
        cause: crate::custom_apps_functions::runtime::EventCause {
            id: envelope.id,
            root_id: envelope.rootid,
            depth: envelope.depth,
            actor: envelope.actor.clone(),
        },
        delivery: info.clone(),
    };
    run_function(job, body, Some(event)).await
}

#[cfg(not(feature = "custom-app-functions"))]
async fn run_listener(
    job: &JobArgs,
    envelope: &oxy_custom_apps::events::OxyEvent,
    info: &deliveries::DeliveryInfo,
) -> Result<FunctionRun, String> {
    let _ = (envelope, info);
    run_function(job, Vec::new(), None).await
}

/// How one run of a listener ended, as the attempt judges it. Not what it
/// answered: that stays out of Oxy's Postgres.
struct ListenerRun<'a> {
    status: &'a str,
    http_status: u16,
    error: Option<&'a str>,
    memory_limit: bool,
}

/// What an attempt that ran counts as, and the error it records (none for
/// `ok`). The SDK's `onEvent` answers 400 for a body that is not an event, so
/// any answer outside 2xx is an error, named by its status alone. A run the
/// platform stopped (its heap limit, a worker shutting down) or never started
/// (shed) is a crash: not the handler's bug.
fn attempt_of(run: &ListenerRun<'_>) -> (AttemptOutcome, Option<String>) {
    let error = |fallback: &str| Some(run.error.unwrap_or(fallback).to_string());
    match run.status {
        "success" if (200..300).contains(&run.http_status) => (AttemptOutcome::Ok, None),
        "success" => (
            AttemptOutcome::Error,
            Some(format!("the listener answered HTTP {}", run.http_status)),
        ),
        "timeout" => (
            AttemptOutcome::Timeout,
            error("function execution timed out"),
        ),
        "error" if run.memory_limit => (
            AttemptOutcome::Crash,
            error("function exceeded its memory limit"),
        ),
        "error" => match run.error.and_then(non_retryable_reason) {
            Some(reason) => (AttemptOutcome::NonRetryable, Some(reason)),
            None => (AttemptOutcome::Error, error("function failed")),
        },
        other => (
            AttemptOutcome::Crash,
            Some(
                run.error
                    .map_or_else(|| format!("the run ended {other}"), str::to_string),
            ),
        ),
    }
}

/// The name the SDK's `NonRetryableError` carries (`@oxy-hq/sdk/events`).
const NON_RETRYABLE: &str = "NonRetryableError";

/// The reason a run's error gives when what the handler threw is named
/// exactly [`NON_RETRYABLE`]: its message, or the name when it has none.
///
/// A thrown error reaches the invocation as `function threw: <its stack>`
/// (`RuntimeError::Js`), the stack headed by the error's own name
/// (`__oxyNamedError` in `bootstrap.js`), or deno's `Uncaught …` message when
/// the stack is one line. Only that head is read: the name quoted in another
/// error's message, or a longer name, is not this one.
fn non_retryable_reason(error: &str) -> Option<String> {
    let thrown = error.strip_prefix("function threw: ")?;
    let thrown = thrown
        .strip_prefix("Uncaught (in promise) ")
        .or_else(|| thrown.strip_prefix("Uncaught "))
        .unwrap_or(thrown);
    let rest = thrown.strip_prefix(NON_RETRYABLE)?;
    let message = match rest.strip_prefix(": ") {
        Some(message) => message,
        None if rest.is_empty() || rest.starts_with('\n') => "",
        None => return None,
    };
    let message = message.split("\n    at ").next().unwrap_or_default().trim();
    Some(if message.is_empty() {
        NON_RETRYABLE.to_string()
    } else {
        message.to_string()
    })
}

impl FunctionRun {
    fn listener_run(&self) -> ListenerRun<'_> {
        ListenerRun {
            status: self.status,
            http_status: self.http_status,
            error: self.error.as_deref(),
            memory_limit: self.memory_limit,
        }
    }
}

#[cfg(test)]
mod tests;
