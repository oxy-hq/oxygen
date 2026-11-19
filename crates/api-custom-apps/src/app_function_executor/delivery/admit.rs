//! Whether an event's listener still takes it, before anything runs
//! (`delivery::deliver`).

use oxy_custom_apps::calls::SendTarget;
use oxy_custom_apps::events::OxyEvent;

use super::event_name;
use crate::app_events::subscriptions;
use crate::app_function_executor::JobArgs;
use crate::custom_apps_functions::queued_admission::QueuedTrigger;

/// Whether the listener runs: [`admit`]'s answer.
pub(super) enum Admitted {
    Run,
    /// Never: kill the delivery with this reason.
    Kill(String),
    /// Not this time (the subscription could not be read): the attempt fails.
    Failed(String),
}

/// Whether `envelope`'s listener still takes it.
///
/// A directed event (`ctx.apps.send`) names its function and has no
/// subscription to check; its admission ("callable") is decided when it runs
/// (`queued_admission`). The name and the task's trigger must agree: a
/// send-named event queued as anything else, or a call-triggered task of an
/// ordinary event or of another function, is killed, not run.
pub(super) async fn admit(job: &JobArgs, envelope: &OxyEvent) -> Admitted {
    let send = SendTarget::parse(&event_name(envelope));
    let calls = !needs_subscription(job.trigger);
    if calls
        && send
            .as_ref()
            .is_some_and(|t| t.function == job.function_name)
    {
        return Admitted::Run;
    }
    if calls || send.is_some() {
        return Admitted::Kill(format!(
            "{} is not a send to {}, but the task says it is one or the name does",
            envelope.event_type, job.function_name
        ));
    }
    match subscriptions::listener_subscribes(
        &job.db,
        job.app_id,
        &job.environment,
        &job.function_name,
        &envelope.event_type,
    )
    .await
    {
        Ok(true) => Admitted::Run,
        Ok(false) => Admitted::Kill(format!(
            "listener no longer subscribes to {}",
            envelope.event_type
        )),
        Err(e) => Admitted::Failed(format!("could not read the listener's subscription: {e}")),
    }
}

/// Whether a delivery run by `trigger` is to a function that must still
/// subscribe to the event: an event's listener does; a call (a delivered send)
/// is to a function that listens to nothing.
pub(super) fn needs_subscription(trigger: QueuedTrigger) -> bool {
    trigger != QueuedTrigger::Call
}
