//! Why a one-off custom-app function job was queued, and the trigger the
//! runner reads back from the task's label.
//!
//! Two closed enums, moved out of `oxy-app` unchanged (domain-boundaries S6
//! step 3). [`FunctionJobTrigger`] is what a caller states when it queues a job;
//! [`QueuedTrigger`] is what the runner reads back from the label. They stay
//! together because [`FunctionJobTrigger::queued`] maps one to the other.

/// Why a one-off function job was enqueued.
///
/// An enum rather than a `&str` because the label is not free text: it is
/// replayed by `app_function_executor` as the invocation `mode`, and any value
/// that executor does not recognise silently becomes `schedule` — a function
/// branching on `mode` would then be told a webhook was a cron fire. Making the
/// set closed here means a new trigger cannot be added without visiting that
/// match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionJobTrigger {
    /// Admin console **Run now**, or the API job trigger.
    Manual,
    /// A verified inbound webhook from a third party.
    Webhook,
    /// **Run now** `as=schedule`: a scheduled function run once as its
    /// schedule would run it — no input, the system identity, its retry
    /// policy (environments design §4.5, ruling 3). Its invocation `mode` is
    /// `manual`: a person asked.
    ScheduleNow,
    /// An event another app published, delivered to a function that declares
    /// it listens (`on`) in its manifest. Its invocation `mode` is `event`.
    Event,
}

impl FunctionJobTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Webhook => "webhook",
            Self::ScheduleNow => "schedule_now",
            Self::Event => "event",
        }
    }

    /// The trigger the runner reads back from the task's label.
    pub fn queued(self) -> QueuedTrigger {
        match self {
            Self::Manual => QueuedTrigger::Manual,
            Self::Webhook => QueuedTrigger::Webhook,
            Self::ScheduleNow => QueuedTrigger::ScheduleNow,
            Self::Event => QueuedTrigger::Event,
        }
    }
}

/// What queued a run, as its task's trigger label says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueuedTrigger {
    /// A cron fire (`scheduled`), and every task queued before the label was
    /// carried, which were all cron fires.
    Cron,
    /// Run now, or a check run (`manual`).
    Manual,
    /// Run now as the function's schedule would run it (`schedule_now`).
    ScheduleNow,
    /// A verified inbound webhook (`webhook`).
    Webhook,
    /// A published event delivered to a listening function (`event`).
    Event,
    /// Another app's `ctx.apps.call`, or a `ctx.apps.send` delivered (`call`).
    Call,
    /// A label this release does not know: queued by a newer one. Never run
    /// (`app_function_executor::read_task` refuses it) — an unknown label used
    /// to read as a cron fire, which ran a function under rules it was not
    /// queued for.
    Unknown,
}

impl QueuedTrigger {
    /// The trigger a task's label names. No label (a task queued before the
    /// field was carried) and `scheduled` are cron fires; a label this release
    /// does not know is [`Self::Unknown`], which fails closed.
    pub fn from_label(label: Option<&str>) -> Self {
        match label {
            Some("manual") => Self::Manual,
            Some("schedule_now") => Self::ScheduleNow,
            Some("webhook") => Self::Webhook,
            Some("event") => Self::Event,
            Some("call") => Self::Call,
            None | Some("scheduled") => Self::Cron,
            Some(_) => Self::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_trigger_round_trips_its_label() {
        assert_eq!(FunctionJobTrigger::Event.as_str(), "event");
        assert_eq!(FunctionJobTrigger::Event.queued(), QueuedTrigger::Event);
        assert_eq!(
            QueuedTrigger::from_label(Some("event")),
            QueuedTrigger::Event
        );
    }

    #[test]
    fn call_is_known_and_a_label_nobody_knows_fails_closed() {
        assert_eq!(QueuedTrigger::from_label(Some("call")), QueuedTrigger::Call);
        assert_eq!(QueuedTrigger::from_label(None), QueuedTrigger::Cron);
        assert_eq!(
            QueuedTrigger::from_label(Some("scheduled")),
            QueuedTrigger::Cron
        );
        for label in ["send", "backfill", "Manual", ""] {
            assert_eq!(
                QueuedTrigger::from_label(Some(label)),
                QueuedTrigger::Unknown,
                "{label:?}"
            );
        }
    }
}
