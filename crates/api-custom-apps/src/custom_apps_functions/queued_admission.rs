//! What a queued function run may be outside production, decided by what
//! queued it (`internal-docs/2026-09-10-custom-app-environments-design.md`
//! §4.5).
//!
//! Production runs every queued run, as it always has. Outside production a
//! queued run is admitted only as one of five things, each by its own rule:
//!
//! | Trigger | Admitted outside production when |
//! | --- | --- |
//! | **manual** (Run now, a check run) | the function is `"check": true` |
//! | **schedule_now** (Run now `as=schedule`) | the function declares a `schedule` — production included; the switch is not asked, it is a person's one-off |
//! | **scheduled** (a cron fire) | it declares a `schedule`, the environment's schedules switch is on, and a sandbox is not past its idle expiry |
//! | **webhook** | it declares a `webhook` block, and a sandbox is not past its idle expiry |
//! | **event** (a published event) | it declares a non-empty `on`, the environment's events switch is on, and a sandbox is not past its idle expiry |
//! | **call** (another app's `ctx.apps.call`) | it declares `"callable": true` (production included), and a sandbox is not past its idle expiry |
//! | **unknown** (a label this release does not know) | never |
//!
//! "Declares" is read from the build the runner resolved, never trusted from
//! whoever queued the task: the environment's pointer may have moved while the
//! task waited. A deleting environment serves no build, so it never gets this
//! far. The rule is pure ([`decide`]); [`environment_state`] reads the
//! facts it needs about the environment, and only for the triggers that ask.

use chrono::Utc;
use entity::{app_environments, apps};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ConnectionTrait, DbErr, EntityTrait};

use crate::custom_apps_env_resolve::ResolvedEnvironment;
use crate::custom_apps_sandboxes::activity;

pub(crate) use oxy_custom_apps::function_trigger::QueuedTrigger;

/// What a function declares in one build's manifest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Declared {
    pub check: bool,
    pub schedule: bool,
    pub webhook: bool,
    /// A non-empty `on`: the function listens for published events.
    pub on: bool,
    /// `"callable": true`: other apps may call the function.
    pub callable: bool,
}

impl Declared {
    /// Read from the function's `manifest_json`. A missing manifest declares
    /// nothing.
    pub(crate) fn of(manifest: Option<&serde_json::Value>) -> Self {
        Self {
            check: super::check_run::manifest_marks_check(manifest),
            schedule: manifest.and_then(declared_schedule).is_some(),
            webhook: manifest
                .and_then(super::function_webhook_contract)
                .is_some(),
            on: manifest.is_some_and(declares_on),
            callable: manifest
                .and_then(|m| m.get("callable"))
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        }
    }
}

/// The cron a function's manifest declares, if any — the same reading the
/// schedule rows are registered from.
pub(crate) fn declared_schedule(manifest: &serde_json::Value) -> Option<&str> {
    manifest.get("schedule").and_then(serde_json::Value::as_str)
}

/// Whether a manifest's `on` names at least one event or table: a non-empty
/// string, a `{ table }` object or a non-empty array. Anything else declares
/// nothing.
fn declares_on(manifest: &serde_json::Value) -> bool {
    match manifest.get("on") {
        Some(serde_json::Value::String(s)) => !s.is_empty(),
        Some(serde_json::Value::Object(o)) => o.contains_key("table"),
        Some(serde_json::Value::Array(a)) => !a.is_empty(),
        _ => false,
    }
}

/// What the rule needs to know of a non-production environment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct EnvironmentState {
    /// `app_environments.schedules_enabled`.
    pub schedules_enabled: bool,
    /// `app_environments.events_enabled`.
    pub events_enabled: bool,
    /// A sandbox idle past its expiry, not yet swept. Never staging.
    pub expired: bool,
}

/// Why a queued run outside production was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueuedRefusal {
    NotACheck,
    NotScheduled,
    SchedulesOff,
    NoWebhook,
    EventsOff,
    NoOn,
    NotCallable,
    UnknownTrigger,
    Expired,
}

/// A queued run refused by [`decide`], in the environment it named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QueuedRefused {
    pub environment: AppEnvironment,
    pub why: QueuedRefusal,
}

impl QueuedRefused {
    /// The run's failure, in one sentence that says what to change.
    pub(crate) fn message(&self) -> String {
        let env = &self.environment;
        match self.why {
            QueuedRefusal::NotACheck => format!(
                "a manual run in the {env} environment runs only a function its build marks \
                 `\"check\": true`; run a scheduled function as its schedule would with \
                 `as=schedule`"
            ),
            QueuedRefusal::NotScheduled => format!(
                "the function declares no `schedule` in the build the {env} environment serves, \
                 so it does not run as a schedule there"
            ),
            QueuedRefusal::SchedulesOff => format!(
                "schedules are off in the {env} environment; turn them on with \
                 `oxyc env schedules <app> {env} --on`"
            ),
            QueuedRefusal::NoWebhook => format!(
                "the function declares no `webhook` block in the build the {env} environment \
                 serves"
            ),
            QueuedRefusal::EventsOff => format!(
                "events are off in this environment ({env}); an operator turns them on per \
                 environment"
            ),
            QueuedRefusal::NoOn => format!(
                "the function declares no `on` in the build the {env} environment serves, so it \
                 receives no events there"
            ),
            QueuedRefusal::NotCallable => format!(
                "the function is not marked `\"callable\": true` in the build the {env} environment \
                 serves"
            ),
            QueuedRefusal::UnknownTrigger => {
                "this release does not know what queued the run".to_string()
            }
            QueuedRefusal::Expired => format!(
                "the sandbox {env} is past its idle expiry; publish to it or call a function in \
                 it to keep it"
            ),
        }
    }
}

/// May a run queued by `trigger` run here? `production` admits everything
/// but a `schedule_now` of a function with no schedule; `state` is the
/// environment's, read by [`environment_state`] — `None` where no rule asks.
pub(crate) fn decide(
    trigger: QueuedTrigger,
    declared: Declared,
    production: bool,
    state: Option<EnvironmentState>,
) -> Result<(), QueuedRefusal> {
    let state = state.unwrap_or_default();
    let refuse_unless = |ok: bool, why| if ok { Ok(()) } else { Err(why) };
    match trigger {
        QueuedTrigger::Unknown => Err(QueuedRefusal::UnknownTrigger),
        QueuedTrigger::ScheduleNow => refuse_unless(declared.schedule, QueuedRefusal::NotScheduled),
        // Callable everywhere it runs, production included; only a sandbox's
        // expiry is read from the environment.
        QueuedTrigger::Call => {
            refuse_unless(declared.callable, QueuedRefusal::NotCallable)?;
            refuse_unless(production || !state.expired, QueuedRefusal::Expired)
        }
        _ if production => Ok(()),
        QueuedTrigger::Manual => refuse_unless(declared.check, QueuedRefusal::NotACheck),
        QueuedTrigger::Cron => {
            refuse_unless(declared.schedule, QueuedRefusal::NotScheduled)?;
            refuse_unless(state.schedules_enabled, QueuedRefusal::SchedulesOff)?;
            refuse_unless(!state.expired, QueuedRefusal::Expired)
        }
        QueuedTrigger::Webhook => {
            refuse_unless(declared.webhook, QueuedRefusal::NoWebhook)?;
            refuse_unless(!state.expired, QueuedRefusal::Expired)
        }
        QueuedTrigger::Event => {
            refuse_unless(declared.on, QueuedRefusal::NoOn)?;
            refuse_unless(state.events_enabled, QueuedRefusal::EventsOff)?;
            refuse_unless(!state.expired, QueuedRefusal::Expired)
        }
    }
}

/// The environment's state, for a cron fire, a webhook or an event outside
/// production — the triggers whose rule reads it. `None` for every other run, which
/// pays no read.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
pub(crate) async fn environment_state<C: ConnectionTrait>(
    db: &C,
    app: &apps::Model,
    resolved: &ResolvedEnvironment,
    trigger: QueuedTrigger,
) -> Result<Option<EnvironmentState>, DbErr> {
    if resolved.is_production()
        || !matches!(
            trigger,
            QueuedTrigger::Cron
                | QueuedTrigger::Webhook
                | QueuedTrigger::Event
                | QueuedTrigger::Call
        )
    {
        return Ok(None);
    }
    let name = resolved.environment.name();
    let Some(row) = app_environments::Entity::find_by_id((app.id, name))
        .one(db)
        .await?
    else {
        return Ok(Some(EnvironmentState::default()));
    };
    let expired = matches!(resolved.environment, AppEnvironment::Dev { .. })
        && activity::is_past_idle_expiry(db, &row, Utc::now()).await?;
    Ok(Some(EnvironmentState {
        schedules_enabled: row.schedules_enabled,
        // Read only for an event: the other triggers never ask.
        events_enabled: trigger == QueuedTrigger::Event && row.events_enabled,
        expired,
    }))
}

#[cfg(test)]
#[path = "queued_admission_tests.rs"]
mod tests;
