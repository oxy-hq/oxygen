//! The automatic rollback of production after a failed `production_smoke`,
//! where the app's `auto_rollback` setting is on.
//!
//! It is the rollback `oxyc rollback production` makes
//! (`custom_apps_promote::act::rollback_unattended`: the same plan, blockers,
//! migrations rule, schedule and subscription reconcile), with the platform
//! as the mover and its own audit row, `app.auto_rolled_back`. And a rollback
//! is a move like any other: it queues a production attempt for the build it
//! rolled back to.
//!
//! **When it does not move**, and says why instead ([`decide`]):
//!
//! | Reason | Why |
//! | --- | --- |
//! | production has moved since | someone promoted or rolled back after the check started; undoing their move is not this attempt's to do. The move itself also refuses it under the app's row lock (`409 target_moved`) |
//! | the failed build reached production by a rollback | a rollback is never rolled back automatically: of two builds that both fail, each would send production to the other, forever |
//! | production served no other build before | there is nothing to go back to |
//! | no baseline from before the move | no error the check found can be called the move's: there was nothing to tell new errors from old ones (`super::baseline::tells_apart_for`, read by the platform, whatever the runner reported). A promoting publish straight to production is the common case |
//! | the rollback's own refusal | a blocker of the rollback path (a build whose validation no longer passes), or a migration that failed |
//!
//! **Once per attempt.** The task that calls this is queued once per
//! production attempt, and what it did is written on the check's row
//! (`detail.rollback`); a row that already says what happened is not acted on
//! again ([`recorded`]).

use chrono::Utc;
use entity::{app_build_checks, app_builds, apps};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};

use super::moved::{self, Move};
use crate::custom_apps_build_checks::kind::cap;
use crate::custom_apps_promote::act;
use crate::custom_apps_promote::unattended_audit::Unattended;

/// The audit action of an automatic rollback.
pub const AUTO_ROLLED_BACK: &str = "app.auto_rolled_back";

/// The longest reason kept for a rollback that did not happen.
const MAX_REASON_CHARS: usize = 300;

/// Why a failed check of a build production served without a baseline is
/// not rolled back.
pub const NO_BASELINE: &str =
    "production had no baseline from before the move to compare its errors with";

/// Why a failed check's move is not rolled back automatically, before any
/// rollback is tried; `None` when it may be. `baselined`: the attempt had a
/// baseline that tells new errors apart, as the platform reads it.
pub fn decide(checked: &Move, baselined: bool) -> Option<&'static str> {
    if !checked.newest {
        return Some("production has moved to another build since the check started");
    }
    if checked.was_rollback() {
        return Some(
            "the build reached production by a rollback, and a rollback is never rolled back \
             automatically",
        );
    }
    if checked.previous.is_none() {
        return Some("production served no other build before this one");
    }
    if !baselined {
        return Some(NO_BASELINE);
    }
    None
}

/// The rollback a check's row already records, if any.
pub fn recorded(check: &app_build_checks::Model) -> Option<Value> {
    check
        .detail
        .as_ref()?
        .get("rollback")
        .filter(|rollback| rollback.is_object())
        .cloned()
}

fn not_rolled_back(reason: &str) -> Value {
    json!({ "rolled_back": false, "reason": cap(reason, MAX_REASON_CHARS),
        "at": Utc::now().fixed_offset() })
}

/// Roll production back from `build`, whose `production_smoke` (`check`)
/// failed, to the build it served before `checked`; or say why not. The
/// answer is what `detail.rollback` records. `baselined` as [`decide`] reads it.
pub async fn roll_back(
    db: &DatabaseConnection,
    (app, build): (&apps::Model, &app_builds::Model),
    check: &app_build_checks::Model,
    checked: &Move,
    baselined: bool,
) -> Value {
    if let Some(why) = decide(checked, baselined) {
        return not_rolled_back(why);
    }
    let why = Unattended {
        action: AUTO_ROLLED_BACK,
        because: json!({
            "reason": "production_smoke failed",
            "check": check.id,
            "attempt": check.attempt,
            "moved_by": checked.by,
        }),
    };
    match act::rollback_unattended(db, app.id, build.id, &why).await {
        Ok(plan) => {
            tracing::warn!(app_id = %app.id, from = %build.build_id, to = %plan.build.build_id,
                attempt = check.attempt, "production was rolled back automatically");
            json!({ "rolled_back": true,
                "to": moved::BuildName { id: plan.build.id, build_id: plan.build.build_id },
                "at": Utc::now().fixed_offset() })
        }
        Err(e) => {
            tracing::warn!(app_id = %app.id, build = %build.build_id, error = %e,
                "production could not be rolled back automatically");
            not_rolled_back(&e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use uuid::Uuid;

    use super::*;

    fn checked(action: &str, previous: Option<Uuid>, newest: bool) -> Move {
        Move {
            action: action.into(),
            by: None,
            at: Utc
                .with_ymd_and_hms(2026, 11, 22, 12, 0, 0)
                .unwrap()
                .fixed_offset(),
            previous,
            newest,
        }
    }

    #[test]
    fn a_promote_production_still_serves_with_a_build_before_it_may_be_rolled_back() {
        let before = Some(Uuid::from_u128(1));
        assert_eq!(decide(&checked("promote", before, true), true), None);
        assert_eq!(decide(&checked("publish", before, true), true), None);
    }

    /// Each reason, and above all: a rollback is never rolled back, so two
    /// failing builds cannot send production back and forth.
    #[test]
    fn a_moved_production_a_rollback_and_a_first_build_are_left_alone() {
        let before = Some(Uuid::from_u128(1));
        let moved_on = decide(&checked("promote", before, false), true).unwrap();
        assert!(moved_on.contains("moved to another build"), "{moved_on}");
        let looped = decide(&checked("rollback", before, true), true).unwrap();
        assert!(
            looped.contains("never rolled back automatically"),
            "{looped}"
        );
        let first = decide(&checked("promote", None, true), true).unwrap();
        assert!(first.contains("no other build"), "{first}");
        // Production having moved is said first: it is the reason that matters.
        assert_eq!(
            decide(&checked("rollback", None, false), true),
            Some(moved_on)
        );
    }

    /// A failed check with no baseline found nothing it can call the move's:
    /// production is never rolled back on it, the move otherwise rollable.
    /// The reasons above are still said first.
    #[test]
    fn a_check_with_no_baseline_never_rolls_production_back() {
        let before = Some(Uuid::from_u128(1));
        assert_eq!(
            decide(&checked("promote", before, true), false),
            Some(NO_BASELINE)
        );
        assert_eq!(
            decide(&checked("publish", before, true), false),
            Some(NO_BASELINE)
        );
        let moved_on = decide(&checked("promote", before, false), false).unwrap();
        assert!(moved_on.contains("moved to another build"), "{moved_on}");
        let first = decide(&checked("promote", None, true), false).unwrap();
        assert!(first.contains("no other build"), "{first}");
    }
}
