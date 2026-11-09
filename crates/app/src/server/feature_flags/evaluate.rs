//! Pure flag evaluation: a flag's state plus who is asking, to a variant and why.
//!
//! No I/O and no globals, so every rule is tested here without a database. The
//! cache (`super::cache`) holds a [`FlagState`] per flag; `super::evaluate` looks it
//! up, calls [`evaluate`], and records a split assignment as an exposure.

use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::registry::{FlagDef, Unit};
use super::targeting::{Resolved, Rules, TOTAL_WEIGHT};

/// Who a flag is evaluated for: OpenFeature's evaluation context, cut to what Oxy
/// targets on. Both fields optional: a request before sign-in has neither, and a
/// flag that needs the missing one serves its off variant (`Reason::Default`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlagContext {
    pub org_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
}

impl FlagContext {
    pub fn org(org_id: Uuid) -> Self {
        Self {
            org_id: Some(org_id),
            user_id: None,
        }
    }

    pub fn member(org_id: Uuid, user_id: Uuid) -> Self {
        Self {
            org_id: Some(org_id),
            user_id: Some(user_id),
        }
    }
}

/// Why a variant was served, in OpenFeature's resolution-reason vocabulary.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Reason {
    /// On or off for everyone: no rules.
    Static,
    /// The kill switch is off.
    Disabled,
    /// A user or org override named this caller.
    TargetingMatch,
    /// The weighted split bucketed this caller. The only reason that is an
    /// experiment exposure: a forced variant is not a random assignment.
    Split,
    /// The fallthrough's one variant, or the off variant because the context lacks
    /// the unit a split needs, or the registry default while the cache is unloaded.
    Default,
    /// The stored rules do not compile against this build's registry; the off
    /// variant is served until an admin fixes them.
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Evaluation {
    pub variant: &'static str,
    pub reason: Reason,
    /// The caller's bucket in `0..10_000`, for a split.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bucket: Option<u32>,
}

impl Evaluation {
    fn of(variant: &'static str, reason: Reason) -> Self {
        Self {
            variant,
            reason,
            bucket: None,
        }
    }
}

/// One flag as the cache holds it.
#[derive(Clone, Debug)]
pub struct FlagState {
    /// The kill switch. Off serves the off variant to everyone, overrides included,
    /// which is what an incident needs.
    pub enabled: bool,
    pub rules: RulesState,
}

#[derive(Clone, Debug)]
pub enum RulesState {
    /// A plain global flag: on is `variants[1]` for everyone.
    None,
    Compiled(Arc<Rules>),
    /// Stored rules this build cannot serve (an unknown variant, unreadable JSON).
    Invalid,
}

impl FlagState {
    pub fn global(enabled: bool) -> Self {
        Self {
            enabled,
            rules: RulesState::None,
        }
    }
}

/// The variant `def` serves `ctx`, given `state`.
pub fn evaluate(def: &FlagDef, state: &FlagState, ctx: &FlagContext) -> Evaluation {
    if !state.enabled {
        return Evaluation::of(def.off(), Reason::Disabled);
    }
    let rules = match &state.rules {
        RulesState::None => return Evaluation::of(def.on(), Reason::Static),
        RulesState::Invalid => return Evaluation::of(def.off(), Reason::Error),
        RulesState::Compiled(rules) => rules,
    };
    let forced = ctx
        .user_id
        .and_then(|id| rules.users.get(&id))
        .or_else(|| ctx.org_id.and_then(|id| rules.orgs.get(&id)));
    if let Some(variant) = forced {
        return Evaluation::of(variant, Reason::TargetingMatch);
    }
    match &rules.fallthrough {
        Resolved::Variant(variant) => Evaluation::of(variant, Reason::Default),
        Resolved::Split(ranges) => split(def, rules, ranges, ctx),
    }
}

fn split(
    def: &FlagDef,
    rules: &Rules,
    ranges: &[(&'static str, u32)],
    ctx: &FlagContext,
) -> Evaluation {
    let Some(unit_id) = unit_id(def, ctx) else {
        return Evaluation::of(def.off(), Reason::Default);
    };
    let bucket = bucket(&rules.salt, unit_id);
    let variant = ranges
        .iter()
        .find(|(_, upper)| bucket < *upper)
        .map(|(variant, _)| *variant)
        // Unreachable for a compiled split, whose last bound is TOTAL_WEIGHT.
        .unwrap_or_else(|| def.off());
    Evaluation {
        variant,
        reason: Reason::Split,
        bucket: Some(bucket),
    }
}

/// The id a split hashes for `def`: the org or the person.
pub fn unit_id(def: &FlagDef, ctx: &FlagContext) -> Option<Uuid> {
    match def.unit {
        Unit::Org => ctx.org_id,
        Unit::User => ctx.user_id,
    }
}

/// `sha256("{salt}.{unit_id}")`, first eight bytes big-endian, mod 10 000. Stable
/// across builds and machines, and pinned by a test: changing it reshuffles every
/// running experiment.
pub fn bucket(salt: &str, unit_id: Uuid) -> u32 {
    let digest = Sha256::digest(format!("{salt}.{unit_id}").as_bytes());
    let mut head = [0u8; 8];
    head.copy_from_slice(&digest[..8]);
    (u64::from_be_bytes(head) % u64::from(TOTAL_WEIGHT)) as u32
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::server::feature_flags::registry::{BOOLEAN, FlagKind};
    use crate::server::feature_flags::targeting::{Fallthrough, Targeting};

    const ON_OFF: FlagDef = FlagDef {
        key: "test_on_off",
        description: "",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        per_org: true,
        client: false,
        kind: FlagKind::Product,
        sunset: None,
    };
    const PER_USER: FlagDef = FlagDef {
        unit: Unit::User,
        ..ON_OFF
    };

    fn org(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    fn state(def: &FlagDef, t: Targeting) -> FlagState {
        FlagState {
            enabled: true,
            rules: RulesState::Compiled(Arc::new(t.compile(def).unwrap())),
        }
    }

    fn rollout(on: u32) -> Targeting {
        Targeting {
            users: BTreeMap::new(),
            orgs: BTreeMap::new(),
            fallthrough: Fallthrough::Split {
                weights: [("on".into(), on), ("off".into(), TOTAL_WEIGHT - on)].into(),
            },
            salt: None,
        }
    }

    #[test]
    fn the_bucket_hash_is_pinned() {
        // Values from Python's hashlib over the same input. If these move, every
        // running split reshuffles: that is a new hash version, not a refactor.
        assert_eq!(bucket("test_on_off", org(1)), 1403);
        assert_eq!(bucket("test_on_off", org(2)), 5788);
        assert_eq!(bucket("other-salt", org(1)), 3243);
    }

    #[test]
    fn a_global_flag_is_on_or_off_for_everyone() {
        let ctx = FlagContext::org(org(1));
        let on = evaluate(&ON_OFF, &FlagState::global(true), &ctx);
        assert_eq!((on.variant, on.reason), ("on", Reason::Static));
        let off = evaluate(&ON_OFF, &FlagState::global(false), &FlagContext::default());
        assert_eq!((off.variant, off.reason), ("off", Reason::Disabled));
    }

    #[test]
    fn the_kill_switch_beats_every_override() {
        let mut t = rollout(TOTAL_WEIGHT);
        t.orgs.insert(org(1), "on".into());
        let mut s = state(&ON_OFF, t);
        s.enabled = false;
        let e = evaluate(&ON_OFF, &s, &FlagContext::org(org(1)));
        assert_eq!((e.variant, e.reason), ("off", Reason::Disabled));
    }

    #[test]
    fn a_user_override_beats_their_orgs_override() {
        let mut t = rollout(0);
        t.orgs.insert(org(1), "off".into());
        t.users.insert(org(9), "on".into());
        let s = state(&ON_OFF, t);
        let me = evaluate(&ON_OFF, &s, &FlagContext::member(org(1), org(9)));
        assert_eq!((me.variant, me.reason), ("on", Reason::TargetingMatch));
        let colleague = evaluate(&ON_OFF, &s, &FlagContext::member(org(1), org(8)));
        assert_eq!(
            (colleague.variant, colleague.reason),
            ("off", Reason::TargetingMatch)
        );
    }

    #[test]
    fn on_for_named_orgs_only() {
        let mut t = rollout(0);
        t.fallthrough = Fallthrough::Variant {
            variant: "off".into(),
        };
        t.orgs.insert(org(1), "on".into());
        let s = state(&ON_OFF, t);
        assert_eq!(
            evaluate(&ON_OFF, &s, &FlagContext::org(org(1))).variant,
            "on"
        );
        let other = evaluate(&ON_OFF, &s, &FlagContext::org(org(2)));
        assert_eq!((other.variant, other.reason), ("off", Reason::Default));
        // No context at all: the fallthrough still applies.
        assert_eq!(
            evaluate(&ON_OFF, &s, &FlagContext::default()).variant,
            "off"
        );
    }

    #[test]
    fn a_split_lands_near_its_weights_and_is_sticky() {
        let s = state(&ON_OFF, rollout(2_500));
        let on = (0..10_000u128)
            .filter(|n| evaluate(&ON_OFF, &s, &FlagContext::org(org(*n))).variant == "on")
            .count();
        assert!((2_250..=2_750).contains(&on), "{on} of 10000 on at 25%");
        let first = evaluate(&ON_OFF, &s, &FlagContext::org(org(42)));
        assert_eq!(evaluate(&ON_OFF, &s, &FlagContext::org(org(42))), first);
        assert_eq!(first.reason, Reason::Split);
    }

    #[test]
    fn raising_a_rollout_never_takes_the_variant_from_anyone_who_had_it() {
        let small = state(&ON_OFF, rollout(1_000));
        let large = state(&ON_OFF, rollout(5_000));
        for n in 0..5_000u128 {
            let ctx = FlagContext::org(org(n));
            if evaluate(&ON_OFF, &small, &ctx).variant == "on" {
                assert_eq!(evaluate(&ON_OFF, &large, &ctx).variant, "on", "org {n}");
            }
        }
    }

    #[test]
    fn a_split_without_its_unit_serves_off() {
        let s = state(&PER_USER, rollout(TOTAL_WEIGHT));
        let e = evaluate(&PER_USER, &s, &FlagContext::org(org(1)));
        assert_eq!((e.variant, e.reason), ("off", Reason::Default));
        let e = evaluate(&PER_USER, &s, &FlagContext::member(org(1), org(2)));
        assert_eq!((e.variant, e.reason), ("on", Reason::Split));
    }

    #[test]
    fn a_per_user_split_can_differ_inside_one_org() {
        let s = state(&PER_USER, rollout(5_000));
        let variants: std::collections::HashSet<_> = (0..64u128)
            .map(|n| evaluate(&PER_USER, &s, &FlagContext::member(org(1), org(n))).variant)
            .collect();
        assert_eq!(variants.len(), 2);
    }

    #[test]
    fn rules_this_build_cannot_serve_serve_off() {
        let s = FlagState {
            enabled: true,
            rules: RulesState::Invalid,
        };
        let e = evaluate(&ON_OFF, &s, &FlagContext::org(org(1)));
        assert_eq!((e.variant, e.reason), ("off", Reason::Error));
    }
}
