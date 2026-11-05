//! A flag's targeting rules: who gets which variant.
//!
//! [`Targeting`] is the wire and storage shape an admin edits. [`Targeting::compile`]
//! checks it against the flag's registry entry and resolves it into [`Rules`], the
//! shape evaluation reads: variant names become the registry's `&'static str`s, and
//! a split becomes cumulative bucket ranges. A rule that does not compile is never
//! served (see `evaluate::RulesState::Invalid`).
//!
//! Evaluation order, first match wins (`evaluate::evaluate`): the kill switch, then a
//! user override, then an org override, then the fallthrough (one variant for
//! everyone else, or a weighted split hashed on the flag's unit).

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::registry::FlagDef;

/// A split's weights are basis points and sum to this: 1 = 0.01%.
pub const TOTAL_WEIGHT: u32 = 10_000;
/// Per override map. A list longer than this is a segment, which is a rule this
/// model does not have yet, not a longer list.
pub const MAX_OVERRIDES: usize = 1_000;
const MAX_SALT_LEN: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Targeting {
    /// Forced variants for named people. Checked before `orgs`, so one person can
    /// preview a variant their org does not get.
    #[serde(default)]
    pub users: BTreeMap<Uuid, String>,
    /// Forced variants for named orgs: a beta list, a customer held back.
    #[serde(default)]
    pub orgs: BTreeMap<Uuid, String>,
    /// What everyone no override names gets.
    pub fallthrough: Fallthrough,
    /// What the split hashes with the unit id. `None` is the flag key. Changing it
    /// reshuffles every assignment, so it is set when a split starts and left alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Fallthrough {
    /// Everyone else gets this variant.
    Variant { variant: String },
    /// Everyone else is bucketed by weight, in basis points summing to
    /// [`TOTAL_WEIGHT`]. A variant left out gets 0.
    Split { weights: BTreeMap<String, u32> },
}

/// [`Targeting`] resolved against its flag: every name is a registry variant.
#[derive(Debug)]
pub struct Rules {
    pub(super) users: HashMap<Uuid, &'static str>,
    pub(super) orgs: HashMap<Uuid, &'static str>,
    pub(super) fallthrough: Resolved,
    pub(super) salt: String,
}

#[derive(Debug)]
pub(super) enum Resolved {
    Variant(&'static str),
    /// `(variant, exclusive upper bound)`, ascending; the last bound is
    /// [`TOTAL_WEIGHT`]. The off variant's range comes LAST, so raising a
    /// treatment's share only ever moves units out of control: a rollout from 10%
    /// to 50% keeps the first 10% where they were.
    Split(Vec<(&'static str, u32)>),
}

impl Targeting {
    /// Check these rules against `def` and resolve them. The error is the reason, in
    /// words an admin can act on.
    pub fn compile(&self, def: &FlagDef) -> Result<Rules, String> {
        Ok(Rules {
            users: overrides(def, "users", &self.users)?,
            orgs: overrides(def, "orgs", &self.orgs)?,
            fallthrough: resolve_fallthrough(def, &self.fallthrough)?,
            salt: salt(def, self.salt.as_deref())?,
        })
    }
}

fn known(def: &FlagDef, name: &str) -> Result<&'static str, String> {
    def.variant(name).ok_or_else(|| {
        format!(
            "\"{name}\" is not a variant of {}; it has {}",
            def.key,
            def.variants.join(", ")
        )
    })
}

fn overrides(
    def: &FlagDef,
    field: &str,
    map: &BTreeMap<Uuid, String>,
) -> Result<HashMap<Uuid, &'static str>, String> {
    if map.len() > MAX_OVERRIDES {
        return Err(format!("{field}: at most {MAX_OVERRIDES} overrides"));
    }
    map.iter()
        .map(|(id, name)| Ok((*id, known(def, name)?)))
        .collect()
}

fn resolve_fallthrough(def: &FlagDef, fallthrough: &Fallthrough) -> Result<Resolved, String> {
    let weights = match fallthrough {
        Fallthrough::Variant { variant } => return Ok(Resolved::Variant(known(def, variant)?)),
        Fallthrough::Split { weights } => weights,
    };
    for name in weights.keys() {
        known(def, name)?;
    }
    let total: u64 = weights.values().map(|w| u64::from(*w)).sum();
    if total != u64::from(TOTAL_WEIGHT) {
        return Err(format!(
            "split weights sum to {total}; they must sum to {TOTAL_WEIGHT} (100.00%)"
        ));
    }
    // Ranges in registry order with the off variant moved last (see `Resolved`).
    let order = def.variants[1..]
        .iter()
        .chain(std::iter::once(&def.variants[0]));
    let mut upper = 0;
    let mut ranges = Vec::new();
    for variant in order {
        let weight = weights.get(*variant).copied().unwrap_or(0);
        if weight > 0 {
            upper += weight;
            ranges.push((*variant, upper));
        }
    }
    Ok(Resolved::Split(ranges))
}

fn salt(def: &FlagDef, salt: Option<&str>) -> Result<String, String> {
    match salt {
        None => Ok(def.key.to_string()),
        Some(s) if s.is_empty() || s.len() > MAX_SALT_LEN => {
            Err(format!("salt must be 1 to {MAX_SALT_LEN} characters"))
        }
        Some(s) => Ok(s.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::feature_flags::registry::{BOOLEAN, Unit};

    const EXPERIMENT: FlagDef = FlagDef {
        key: "test_experiment",
        description: "",
        default_enabled: false,
        variants: &["control", "a", "b"],
        unit: Unit::Org,
        per_org: true,
        client: false,
    };
    const ON_OFF: FlagDef = FlagDef {
        key: "test_on_off",
        description: "",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        per_org: true,
        client: false,
    };

    fn split(weights: &[(&str, u32)]) -> Targeting {
        Targeting {
            users: BTreeMap::new(),
            orgs: BTreeMap::new(),
            fallthrough: Fallthrough::Split {
                weights: weights.iter().map(|(v, w)| (v.to_string(), *w)).collect(),
            },
            salt: None,
        }
    }

    #[test]
    fn a_split_puts_control_last_so_a_rollout_only_grows_out_of_it() {
        let rules = split(&[("control", 5_000), ("a", 3_000), ("b", 2_000)])
            .compile(&EXPERIMENT)
            .unwrap();
        let Resolved::Split(ranges) = rules.fallthrough else {
            panic!("expected a split");
        };
        assert_eq!(
            ranges,
            vec![("a", 3_000), ("b", 5_000), ("control", 10_000)]
        );
    }

    #[test]
    fn zero_weights_take_no_range() {
        let rules = split(&[("on", 10_000), ("off", 0)])
            .compile(&ON_OFF)
            .unwrap();
        let Resolved::Split(ranges) = rules.fallthrough else {
            panic!("expected a split");
        };
        assert_eq!(ranges, vec![("on", 10_000)]);
    }

    #[test]
    fn weights_must_sum_to_one_hundred_percent() {
        let err = split(&[("on", 5_000)]).compile(&ON_OFF).unwrap_err();
        assert!(err.contains("sum to 5000"), "{err}");
        assert!(split(&[]).compile(&ON_OFF).is_err());
        assert!(
            split(&[("on", u32::MAX), ("off", 1)])
                .compile(&ON_OFF)
                .is_err()
        );
    }

    #[test]
    fn an_unknown_variant_is_refused_wherever_it_appears() {
        assert!(split(&[("treatment", 10_000)]).compile(&ON_OFF).is_err());

        let mut t = split(&[("on", 10_000)]);
        t.orgs.insert(Uuid::from_u128(1), "beta".into());
        let err = t.compile(&ON_OFF).unwrap_err();
        assert!(err.contains("\"beta\" is not a variant"), "{err}");

        let mut t = split(&[("on", 10_000)]);
        t.fallthrough = Fallthrough::Variant {
            variant: "maybe".into(),
        };
        assert!(t.compile(&ON_OFF).is_err());
    }

    #[test]
    fn the_salt_defaults_to_the_key_and_is_bounded() {
        let mut t = split(&[("on", 10_000)]);
        assert_eq!(t.compile(&ON_OFF).unwrap().salt, "test_on_off");
        t.salt = Some(String::new());
        assert!(t.compile(&ON_OFF).is_err());
        t.salt = Some("x".repeat(65));
        assert!(t.compile(&ON_OFF).is_err());
        t.salt = Some("2026-11-relaunch".into());
        assert_eq!(t.compile(&ON_OFF).unwrap().salt, "2026-11-relaunch");
    }

    #[test]
    fn too_many_overrides_are_refused() {
        let mut t = split(&[("on", 10_000)]);
        t.orgs = (0..=MAX_OVERRIDES as u128)
            .map(|i| (Uuid::from_u128(i), "on".to_string()))
            .collect();
        assert!(t.compile(&ON_OFF).is_err());
    }

    #[test]
    fn the_wire_shape_is_stable() {
        let json = serde_json::json!({
            "orgs": { "00000000-0000-0000-0000-000000000001": "on" },
            "fallthrough": { "type": "split", "weights": { "on": 2500, "off": 7500 } }
        });
        let t: Targeting = serde_json::from_value(json).unwrap();
        assert!(t.users.is_empty());
        assert_eq!(t.orgs[&Uuid::from_u128(1)], "on");
        assert!(t.compile(&ON_OFF).is_ok());
        let back: Targeting = serde_json::from_value(serde_json::to_value(&t).unwrap()).unwrap();
        assert_eq!(back, t);
    }
}
