//! What refuses an act, and what only warns — decided from facts the plan has
//! already read, with no database in sight (environments design §5.5a,
//! ruling 9).
//!
//! | Blocker | When |
//! | --- | --- |
//! | `staging_never_served` | production moves to a build staging never served (invariant 1) |
//! | `validation_not_passed` | production moves to a build whose validation is not `passed` |
//! | `required_secrets_unset` | a production **promote** to a build declaring a `required` key production has no value for. A rollback, the emergency path, only warns |
//! | `schedule_removal_unconfirmed` | production's move would retire a registered schedule the request did not name in `confirm_schedule_removals` — a rollback too |
//! | `same_person_promote` | a production **promote** by the identity that published the build, while the app requires a second person (`second_person`). A rollback, the emergency path, is exempt when production served its target before — every `oxyc rollback` target did; the console's rollback to a build production never served is refused by the production move itself |
//!
//! A promoting publish (`oxyc publish --promote`) meets the same table as a
//! promote, before anything of it lands (`custom_apps_publish::production_gate`).

use super::error::{
    Blocker, REQUIRED_SECRETS_UNSET, SAME_PERSON_PROMOTE, SCHEDULE_REMOVAL_UNCONFIRMED,
    STAGING_NEVER_SERVED, VALIDATION_NOT_PASSED,
};

use crate::custom_apps_build_state::{PromoteRefusal, Validation};

/// Which verb is planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Promote,
    Rollback,
}

impl Act {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Promote => "promote",
            Self::Rollback => "rollback",
        }
    }
}

/// What a production move's checks read. A staging move has none of these:
/// it is a pointer move that never blocks.
#[derive(Debug, Clone, Copy)]
pub struct ProductionFacts<'a> {
    pub act: Act,
    /// The build's publish id, for the sentences.
    pub build_label: &'a str,
    pub staging_served: bool,
    pub validation: &'a Validation,
    pub required_unset: &'a [String],
    /// Registered schedules the move would retire, by function.
    pub schedule_removals: &'a [String],
    /// `confirm_schedule_removals`, as the request named them.
    pub confirmed_removals: &'a [String],
    /// `apps.require_second_person_to_promote`.
    pub second_person_required: bool,
    /// Whoever moves production is the identity that published the build
    /// (`second_person::Identity::is`).
    pub mover_published: bool,
}

/// The blockers of a production move, in the order of the table above, and
/// the warnings that do not block.
pub fn evaluate(facts: ProductionFacts<'_>) -> (Vec<Blocker>, Vec<String>) {
    let mut blockers = Vec::new();
    let mut warnings = Vec::new();
    if !facts.staging_served {
        blockers.push(staging_never_served(facts.build_label));
    }
    if let Err(PromoteRefusal::ValidationNotPassed { status }) = facts.validation.check() {
        blockers.push(validation_not_passed(facts.build_label, &status));
    }
    if !facts.required_unset.is_empty() {
        let keys = facts.required_unset.join(", ");
        match facts.act {
            Act::Promote => blockers.push(Blocker {
                code: REQUIRED_SECRETS_UNSET,
                message: format!(
                    "the build declares required keys production has no value for: {keys} — \
                     set them first, in the console's Secrets panel on Production (`oxyc env \
                     secret` sets a sandbox's only)"
                ),
            }),
            Act::Rollback => warnings.push(format!(
                "the target declares required keys production has no value for: {keys}"
            )),
        }
    }
    let unconfirmed: Vec<&str> = facts
        .schedule_removals
        .iter()
        .filter(|name| !facts.confirmed_removals.contains(*name))
        .map(String::as_str)
        .collect();
    if !unconfirmed.is_empty() {
        blockers.push(Blocker {
            code: SCHEDULE_REMOVAL_UNCONFIRMED,
            message: format!(
                "this retires the schedule of {}: name each to confirm \
                 (`--remove-schedule <function>`, or `confirm_schedule_removals`)",
                unconfirmed.join(", ")
            ),
        });
    }
    if facts.act == Act::Promote && facts.second_person_required && facts.mover_published {
        blockers.push(same_person_promote(facts.build_label));
    }
    (blockers, warnings)
}

/// Invariant 1: production only ever serves a build staging has served.
pub fn staging_never_served(build_label: &str) -> Blocker {
    Blocker {
        code: STAGING_NEVER_SERVED,
        message: format!(
            "staging has never served build {build_label:?}, and production only serves a build \
             staging has served — publish it to staging (or promote it there from its sandbox) \
             first"
        ),
    }
}

pub fn validation_not_passed(build_label: &str, status: &str) -> Blocker {
    Blocker {
        code: VALIDATION_NOT_PASSED,
        message: format!(
            "build {build_label:?} has validation status {status:?}, not \"passed\" — it cannot \
             be made live"
        ),
    }
}

/// The four-eyes rule, on a rollback to a build production never served:
/// moving production to it is a promote, and its publisher may not.
pub fn same_person_rollback(build_label: &str) -> Blocker {
    Blocker {
        code: SAME_PERSON_PROMOTE,
        message: format!(
            "this app requires a second person to promote, and production has never served \
             build {build_label:?}, so moving production to it is a promote — by the identity \
             that published it (the same user, CI workflow or token). Someone else moves it, or \
             roll back to a build production served before"
        ),
    }
}

/// The four-eyes rule: someone other than the build's publisher promotes it.
pub fn same_person_promote(build_label: &str) -> Blocker {
    Blocker {
        code: SAME_PERSON_PROMOTE,
        message: format!(
            "this app requires a second person to promote, and build {build_label:?} was \
             published by the identity promoting it (the same user, CI workflow or token) — \
             someone else promotes it (`oxyc promote`, or the console's Promote)"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts<'a>(act: Act) -> ProductionFacts<'a> {
        ProductionFacts {
            act,
            build_label: "b-7",
            staging_served: true,
            validation: &Validation::Passed,
            required_unset: &[],
            schedule_removals: &[],
            confirmed_removals: &[],
            second_person_required: false,
            mover_published: false,
        }
    }

    fn codes(blockers: &[Blocker]) -> Vec<&'static str> {
        blockers.iter().map(|b| b.code).collect()
    }

    #[test]
    fn a_build_staging_served_with_nothing_unset_has_no_blocker() {
        let (blockers, warnings) = evaluate(facts(Act::Promote));
        assert!(blockers.is_empty() && warnings.is_empty());
    }

    #[test]
    fn invariant_one_and_validation_block_both_verbs() {
        for act in [Act::Promote, Act::Rollback] {
            let (blockers, _) = evaluate(ProductionFacts {
                staging_served: false,
                validation: &Validation::of("failed"),
                ..facts(act)
            });
            assert_eq!(
                codes(&blockers),
                vec![STAGING_NEVER_SERVED, VALIDATION_NOT_PASSED],
                "{act:?}"
            );
        }
    }

    /// An unset required key blocks a promote; a rollback, the emergency
    /// path, goes ahead and says so.
    #[test]
    fn an_unset_required_key_blocks_a_promote_and_warns_a_rollback() {
        let unset = ["STRIPE_KEY".to_string()];
        let (blockers, warnings) = evaluate(ProductionFacts {
            required_unset: &unset,
            ..facts(Act::Promote)
        });
        assert_eq!(codes(&blockers), vec![REQUIRED_SECRETS_UNSET]);
        assert!(warnings.is_empty());
        let (blockers, warnings) = evaluate(ProductionFacts {
            required_unset: &unset,
            ..facts(Act::Rollback)
        });
        assert!(blockers.is_empty());
        assert!(warnings[0].contains("STRIPE_KEY"), "{warnings:?}");
    }

    /// Every removed schedule must be named; naming one of two is not enough,
    /// for either verb.
    #[test]
    fn each_removed_schedule_must_be_confirmed_by_name() {
        let removals = ["nightly".to_string(), "hourly".to_string()];
        let partly = ["nightly".to_string()];
        for act in [Act::Promote, Act::Rollback] {
            let (blockers, _) = evaluate(ProductionFacts {
                schedule_removals: &removals,
                confirmed_removals: &partly,
                ..facts(act)
            });
            assert_eq!(codes(&blockers), vec![SCHEDULE_REMOVAL_UNCONFIRMED]);
            assert!(blockers[0].message.contains("hourly"));
            assert!(!blockers[0].message.contains("nightly"));
            let (blockers, _) = evaluate(ProductionFacts {
                schedule_removals: &removals,
                confirmed_removals: &removals,
                ..facts(act)
            });
            assert!(blockers.is_empty(), "{act:?}");
        }
    }

    /// The four-eyes rule: promote × rule on/off × same/different mover. Only
    /// a promote by its publisher with the rule on is refused.
    #[test]
    fn a_promote_by_its_publisher_is_refused_only_while_the_rule_is_on() {
        for (rule, same, refused) in [
            (true, true, true),
            (true, false, false),
            (false, true, false),
            (false, false, false),
        ] {
            let (blockers, warnings) = evaluate(ProductionFacts {
                second_person_required: rule,
                mover_published: same,
                ..facts(Act::Promote)
            });
            let expected = if refused {
                vec![SAME_PERSON_PROMOTE]
            } else {
                vec![]
            };
            assert_eq!(codes(&blockers), expected, "rule {rule}, same {same}");
            assert!(warnings.is_empty());
        }
    }

    /// A planned rollback is the emergency path: its target is one production
    /// served (`resolve`, ruling 6), so the rule never refuses it, nor warns
    /// about it. A rollback to a build production never served is refused by
    /// the production move (`second_person::refuse_same_person`).
    #[test]
    fn a_rollback_is_exempt_from_the_second_person_rule() {
        let (blockers, warnings) = evaluate(ProductionFacts {
            second_person_required: true,
            mover_published: true,
            ..facts(Act::Rollback)
        });
        assert!(blockers.is_empty() && warnings.is_empty());
    }

    /// The sentence names the build and the way forward.
    #[test]
    fn the_second_person_blocker_names_the_build_and_who_may_promote() {
        let blocker = same_person_promote("b-7");
        assert_eq!(blocker.code, SAME_PERSON_PROMOTE);
        assert!(blocker.message.contains("\"b-7\""), "{}", blocker.message);
        assert!(
            blocker.message.contains("someone else"),
            "{}",
            blocker.message
        );
    }
}
