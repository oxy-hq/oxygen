use serde_json::json;

use super::*;

const ALL: [QueuedTrigger; 5] = [
    QueuedTrigger::Event,
    QueuedTrigger::Cron,
    QueuedTrigger::Manual,
    QueuedTrigger::ScheduleNow,
    QueuedTrigger::Webhook,
];

fn declared(check: bool, schedule: bool, webhook: bool) -> Declared {
    Declared {
        check,
        schedule,
        webhook,
        on: false,
    }
}

fn state(schedules_enabled: bool, expired: bool) -> Option<EnvironmentState> {
    Some(EnvironmentState {
        schedules_enabled,
        expired,
        ..EnvironmentState::default()
    })
}

/// Production is unchanged: a cron fire, a webhook and Run now run whatever
/// the function declares. The one new refusal is `as=schedule` of a function
/// with no schedule, a trigger that did not exist before.
#[test]
fn production_admits_every_trigger_it_always_did() {
    let nothing = Declared::default();
    for trigger in [
        QueuedTrigger::Cron,
        QueuedTrigger::Manual,
        QueuedTrigger::Webhook,
    ] {
        assert_eq!(decide(trigger, nothing, true, None), Ok(()), "{trigger:?}");
    }
    assert_eq!(
        decide(QueuedTrigger::ScheduleNow, nothing, true, None),
        Err(QueuedRefusal::NotScheduled)
    );
    assert_eq!(
        decide(
            QueuedTrigger::ScheduleNow,
            declared(false, true, false),
            true,
            None
        ),
        Ok(())
    );
}

/// Outside production a manual run is a check and nothing else.
#[test]
fn a_manual_run_outside_production_is_a_check() {
    let on = state(true, false);
    assert_eq!(
        decide(
            QueuedTrigger::Manual,
            declared(true, false, false),
            false,
            None
        ),
        Ok(())
    );
    assert_eq!(
        decide(
            QueuedTrigger::Manual,
            declared(false, true, true),
            false,
            on
        ),
        Err(QueuedRefusal::NotACheck),
        "a scheduled function runs by hand only as its schedule (`as=schedule`)"
    );
}

/// Run now `as=schedule` needs a declared schedule and nothing else: the
/// switch is off and the run still goes, because a person asked for it.
#[test]
fn schedule_now_needs_a_schedule_and_ignores_the_switch() {
    let scheduled = declared(false, true, false);
    for environment_state in [None, state(false, false), state(false, true)] {
        assert_eq!(
            decide(
                QueuedTrigger::ScheduleNow,
                scheduled,
                false,
                environment_state
            ),
            Ok(())
        );
    }
    assert_eq!(
        decide(
            QueuedTrigger::ScheduleNow,
            declared(true, false, true),
            false,
            None
        ),
        Err(QueuedRefusal::NotScheduled),
        "a check is not a schedule"
    );
}

/// A cron fire outside production: a declared schedule, the switch on, and a
/// sandbox not past its expiry — in that order of refusal.
#[test]
fn a_cron_fire_outside_production_needs_the_switch_on_and_no_expiry() {
    let scheduled = declared(false, true, false);
    let fire = |s| decide(QueuedTrigger::Cron, scheduled, false, s);
    assert_eq!(fire(state(true, false)), Ok(()));
    assert_eq!(fire(state(false, false)), Err(QueuedRefusal::SchedulesOff));
    assert_eq!(fire(state(true, true)), Err(QueuedRefusal::Expired));
    assert_eq!(
        fire(None),
        Err(QueuedRefusal::SchedulesOff),
        "a state that was never read is off"
    );
    assert_eq!(
        decide(
            QueuedTrigger::Cron,
            declared(true, false, true),
            false,
            state(true, false)
        ),
        Err(QueuedRefusal::NotScheduled),
        "the build the runner resolved must still declare the schedule"
    );
}

/// A webhook outside production: the block in the resolved build, and a
/// sandbox not past its expiry. The schedules switch has nothing to say.
#[test]
fn a_webhook_outside_production_needs_its_block_and_no_expiry() {
    let hooked = declared(false, false, true);
    let deliver = |s| decide(QueuedTrigger::Webhook, hooked, false, s);
    assert_eq!(deliver(state(false, false)), Ok(()));
    assert_eq!(deliver(state(true, true)), Err(QueuedRefusal::Expired));
    assert_eq!(
        decide(
            QueuedTrigger::Webhook,
            declared(true, true, false),
            false,
            None
        ),
        Err(QueuedRefusal::NoWebhook)
    );
}

fn listening() -> Declared {
    Declared {
        on: true,
        ..Declared::default()
    }
}

fn events_state(events_enabled: bool, expired: bool) -> Option<EnvironmentState> {
    Some(EnvironmentState {
        events_enabled,
        expired,
        ..EnvironmentState::default()
    })
}

#[test]
fn event_is_admitted_in_production() {
    assert_eq!(
        decide(QueuedTrigger::Event, Declared::default(), true, None),
        Ok(())
    );
}

#[test]
fn event_is_refused_outside_production_when_the_switch_is_off() {
    let refusal = decide(
        QueuedTrigger::Event,
        listening(),
        false,
        events_state(false, false),
    );
    assert_eq!(refusal, Err(QueuedRefusal::EventsOff));
    let refused = QueuedRefused {
        environment: AppEnvironment::Staging,
        why: QueuedRefusal::EventsOff,
    };
    assert!(
        refused
            .message()
            .contains("events are off in this environment")
    );
}

#[test]
fn event_is_admitted_outside_production_when_the_switch_is_on_and_on_is_declared() {
    assert_eq!(
        decide(
            QueuedTrigger::Event,
            listening(),
            false,
            events_state(true, false)
        ),
        Ok(())
    );
}

#[test]
fn event_is_refused_when_the_function_declares_no_on() {
    assert_eq!(
        decide(
            QueuedTrigger::Event,
            Declared::default(),
            false,
            events_state(true, false)
        ),
        Err(QueuedRefusal::NoOn)
    );
}

#[test]
fn event_is_refused_in_a_sandbox_past_its_idle_expiry() {
    assert_eq!(
        decide(
            QueuedTrigger::Event,
            listening(),
            false,
            events_state(true, true)
        ),
        Err(QueuedRefusal::Expired)
    );
}

#[test]
fn on_is_read_from_the_manifest() {
    for (on, declares) in [
        (json!("a/b"), true),
        (json!(["a/b"]), true),
        (json!(""), false),
        (json!([]), false),
        (json!(5), false),
    ] {
        assert_eq!(
            Declared::of(Some(&json!({ "on": on.clone() }))).on,
            declares,
            "{on}"
        );
    }
}

/// A check outside production is admitted for a manual run only: being a
/// check opens no cron fire and no webhook.
#[test]
fn being_a_check_opens_no_other_trigger() {
    let check = declared(true, false, false);
    let off = state(false, false);
    assert_eq!(
        decide(QueuedTrigger::Cron, check, false, off),
        Err(QueuedRefusal::NotScheduled)
    );
    assert_eq!(
        decide(QueuedTrigger::Webhook, check, false, off),
        Err(QueuedRefusal::NoWebhook)
    );
}

#[test]
fn every_label_reads_as_its_trigger_and_an_unknown_one_as_a_cron_fire() {
    assert_eq!(
        QueuedTrigger::from_label(Some("manual")),
        QueuedTrigger::Manual
    );
    assert_eq!(
        QueuedTrigger::from_label(Some("schedule_now")),
        QueuedTrigger::ScheduleNow
    );
    assert_eq!(
        QueuedTrigger::from_label(Some("webhook")),
        QueuedTrigger::Webhook
    );
    for cron in [None, Some("scheduled"), Some("backfill"), Some("Manual")] {
        assert_eq!(
            QueuedTrigger::from_label(cron),
            QueuedTrigger::Cron,
            "{cron:?}"
        );
    }
    assert_eq!(
        QueuedTrigger::from_label(Some("event")),
        QueuedTrigger::Event
    );
    assert_eq!(ALL.len(), 5);
}

#[test]
fn what_a_function_declares_is_read_from_its_manifest() {
    let full = json!({
        "check": true,
        "schedule": "0 * * * *",
        "webhook": { "secretVar": "SIGNING", "signatureHeader": "x-sig" },
    });
    assert_eq!(Declared::of(Some(&full)), declared(true, true, true));
    assert_eq!(Declared::of(None), Declared::default());
    // A schedule that is not a cron string, and a webhook block missing what
    // the route verifies with, declare nothing.
    let partial = json!({ "schedule": 5, "webhook": { "secretVar": "SIGNING" } });
    assert_eq!(Declared::of(Some(&partial)), Declared::default());
}

/// Each refusal says what to change, naming the environment.
#[test]
fn every_refusal_names_its_environment() {
    for why in [
        QueuedRefusal::NotACheck,
        QueuedRefusal::NotScheduled,
        QueuedRefusal::SchedulesOff,
        QueuedRefusal::NoWebhook,
        QueuedRefusal::Expired,
        QueuedRefusal::EventsOff,
        QueuedRefusal::NoOn,
    ] {
        let refused = QueuedRefused {
            environment: AppEnvironment::Dev {
                handle: "a1".into(),
            },
            why,
        };
        assert!(refused.message().contains("dev-a1"), "{why:?}");
    }
}
