//! `router::recovery` as the driver of a queued custom-app ask.
//!
//! Nothing enqueues one yet. The executor is registered a release ahead of the
//! handler's enqueue precisely so that, when a serve replica starts queueing
//! asks, no driver is left that fails the kind as unknown or declines it for
//! ever. These pin the two things that would make that false.

use agentic_pipeline::recovery::{STRANDED_GRACE_SECS, may_drive};

use super::super::drive_policy::{IdeDeferral, drive_policy_for};
use super::*;
use crate::server::api::projects::agent_ask::task::AGENT_ASK_KIND;
use crate::server::role_manifest::Role;

/// The `source_type` a queued ask's run carries, and so what the placement
/// gate matches on: the start inserts it as an analytics run. Not
/// `AGENT_ASK_KIND`, which is only the queue spec's discriminator.
const ASK_SOURCE_TYPE: &str = "analytics";

const EVERY_ROLE: [Role; 4] = [Role::All, Role::Ide, Role::Worker, Role::Serve];
const EVERY_DEFERRAL: [IdeDeferral; 4] = [
    IdeDeferral {
        airway: false,
        queue_work: false,
    },
    IdeDeferral {
        airway: true,
        queue_work: false,
    },
    IdeDeferral {
        airway: false,
        queue_work: true,
    },
    IdeDeferral {
        airway: true,
        queue_work: true,
    },
];

fn drives(role: Role, defer: IdeDeferral, unclaimed_secs: u64) -> bool {
    may_drive(
        Some(ASK_SOURCE_TYPE),
        unclaimed_secs,
        drive_policy_for(role, defer),
    )
}

/// The registry every driver loop injects must know the kind, or the pod that
/// claims a queued ask fails it as an unknown `Custom` task.
#[test]
fn the_driver_registry_executes_queued_asks() {
    let registry = build_custom_task_registry(
        &sea_orm::DatabaseConnection::default(),
        &PreaggCacheCtx::default(),
        None,
    );
    assert!(
        registry.get(AGENT_ASK_KIND).is_some(),
        "`build_custom_task_registry` does not register {AGENT_ASK_KIND}"
    );
}

/// No role may decline an ask for ever, under any gate: on a single-process
/// install there is no other node to take it. A deferring `ide` may leave a
/// fresh one for the worker fleet — that is placement — but must take it once
/// nobody has for the grace.
#[test]
fn no_role_declines_a_queued_ask_for_ever() {
    for role in EVERY_ROLE {
        for defer in EVERY_DEFERRAL {
            assert!(
                drives(role, defer, STRANDED_GRACE_SECS),
                "{role:?} with {defer:?} still declines an ask nobody claimed \
                 for the grace: it would never run there"
            );
        }
    }
}

/// Where an ask lands when it is fresh: every role takes it at once, except an
/// `ide` told to leave queue work for the fleet.
#[test]
fn only_an_ide_deferring_queue_work_leaves_a_fresh_ask_for_the_fleet() {
    for role in EVERY_ROLE {
        for defer in EVERY_DEFERRAL {
            let leaves = matches!(role, Role::Ide) && defer.queue_work;
            assert_eq!(drives(role, defer, 0), !leaves, "{role:?} with {defer:?}");
        }
    }
}
