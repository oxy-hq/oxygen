//! Code-declared feature flag registry.
//!
//! Adding a flag: append a `FlagDef` entry below. Removing a flag: delete
//! the entry; any DB row with that key becomes stale and is filtered out
//! at cache load (see `cache::load_map`). Leave that row in place: a rollback
//! to a build that still declares the flag reads its value back, where a
//! deleted row would read the default. Renaming: treat as remove + add.
//!
//! An A/B test is a flag with its own `variants` (`&["control", "treatment"]`),
//! split by the admin's targeting rules (`super::targeting`). Treat a running
//! split as immutable: changing its variants, salt or unit reshuffles who gets
//! what, so a changed experiment is a new flag.
//!
//! A **rollout gate** is a flag of [`FlagKind::Rollout`]: it switches a change on
//! for one deployment at a time while it is proven, and its end state is the
//! change on and the flag deleted. Every one names a `sunset`; the test below
//! fails once that is within [`SUNSET_MARGIN_DAYS`], so the retirement PR lands
//! before the gate expires. `internal-docs/feature-flags.md`, "Rollout gates and
//! their sunset", says how to retire one.

/// The two variants of an on/off flag. Every flag's first variant is its "off"
/// (or control): what a disabled flag, a missing context or a broken rule serves.
pub const BOOLEAN: &[&str] = &["off", "on"];

/// What a split hashes, so everyone sharing one sees the same variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    /// The org: everyone in an account sees the same variant. The default, since
    /// an account that disagrees with itself is a support ticket, and the org is
    /// what an Oxy outcome is measured on.
    Org,
    /// The person, for a change one person sees alone.
    User,
}

/// Why a flag exists, which decides how long it may.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlagKind {
    /// A product switch or an A/B test: lives as long as the choice does.
    Product,
    /// A temporary gate on a change being rolled out deployment by deployment.
    /// Must name a `sunset`; retired by turning the change on and deleting the
    /// flag (`super::rollout`).
    Rollout,
}

/// A [`FlagKind::Rollout`] flag fails the registry test once today is this
/// close to its sunset, so the retirement PR has time to land.
pub const SUNSET_MARGIN_DAYS: i64 = 14;

pub struct FlagDef {
    pub key: &'static str,
    pub description: &'static str,
    /// The value with no row, and the value while the cache is unloaded. With
    /// `variants` beyond two, `true` reads as `variants[1]`.
    pub default_enabled: bool,
    /// At least two, `variants[0]` the off/control variant. [`BOOLEAN`] for an
    /// on/off flag.
    pub variants: &'static [&'static str],
    /// What a weighted split buckets on.
    pub unit: Unit,
    /// Whether every read site passes the org (`is_enabled_for` / `variant`), so
    /// overrides and splits apply. A flag read only with `is_enabled` has no org to
    /// match, so the admin API refuses rules on it rather than store rules that do
    /// nothing. Turn on in the PR that moves the last read site.
    pub per_org: bool,
    /// Whether the web app may read this flag's evaluated variant
    /// (`GET /orgs/{org_id}/feature-flags`). Off for a server-only switch.
    pub client: bool,
    /// Product switch, or a rollout gate with an end date.
    pub kind: FlagKind,
    /// When a [`FlagKind::Rollout`] flag is to be gone: a date (`2027-01-31`) or
    /// a crate version (`0.6.0`). `None` for a product flag.
    pub sunset: Option<&'static str>,
}

pub static FLAGS: &[FlagDef] = &[
    FlagDef {
        key: "billing",
        description: "Master switch for billing & subscription enforcement. \
                      When off, paywall and subscription guards are skipped \
                      for all orgs.",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        per_org: false,
        client: false,
        kind: FlagKind::Product,
        sunset: None,
    },
    FlagDef {
        key: super::APP_EVENTS,
        description: "App events for an org: its apps' Oxy Functions may publish typed events \
                      (ctx.events.publish, tx.events.publish) into the org's OLTP outbox. Off, \
                      every publish is refused EventsNotEnabled before anything is written. \
                      Turn it on per org with an org override; the outbox needs the org's \
                      OLTP store provisioned.",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        // The host reads it with the publishing app's org (`is_enabled_for`).
        per_org: true,
        client: false,
        kind: FlagKind::Product,
        sunset: None,
    },
    FlagDef {
        key: super::REACT_RUNTIME,
        description: "Run opted-in analytics agents (runtime: react) on the react runtime. \
                      Internal and benchmark orgs only.",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        // Read by the served chat start with the workspace's org (`is_enabled_for`).
        per_org: true,
        client: false,
        kind: FlagKind::Product,
        sunset: None,
    },
    FlagDef {
        key: super::REACT_MEMORY,
        description: "Give react runs per-user memory: the user's notes for the workspace in \
                      the prompt, and a reflection turn that may save new ones (one extra \
                      model turn after an answer that took 3+ queries). Needs react_runtime. \
                      Internal and benchmark orgs only.",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        // Read by the served react build with the workspace's org (`is_enabled_for`).
        per_org: true,
        client: false,
        kind: FlagKind::Product,
        sunset: None,
    },
    FlagDef {
        key: super::rollout::COMPILE_REFUSE_FILE_CREDENTIALS_FLAG,
        description: "Fail, instead of warn on, a compile whose config.yml reads a key file \
                      from the repository (BigQuery key_path, Snowflake private-key path) on \
                      a cloud role. The previous revision keeps serving; the replacement is \
                      key_json_var / private_key_var. Check production for workspaces that \
                      still declare a key file before turning this on. Was \
                      OXY_COMPILE_REFUSE_FILE_CREDENTIALS=1, which still overrides this flag \
                      for one release when set.",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        per_org: false,
        client: false,
        kind: FlagKind::Rollout,
        sunset: Some("2027-01-31"),
    },
    FlagDef {
        key: super::rollout::COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS_FLAG,
        description: "Fail, instead of warn on, a compile of an automation that carries a \
                      retired step: export:, cache: or agent_ref: __builder__. The runtime \
                      already ignores the two writes; Demo workspaces seeded before the demo \
                      project dropped its export: blocks still carry them, so check them \
                      first. Was OXY_COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS=1, which still \
                      overrides this flag for one release when set.",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        per_org: false,
        client: false,
        kind: FlagKind::Rollout,
        sunset: Some("2027-01-31"),
    },
    FlagDef {
        key: super::rollout::COMPILE_REFUSE_CLASSIC_AGENTS_FLAG,
        description: "Fail, instead of warn on, a compile that finds a classic *.agent.yml \
                      file or an automation agent_ref naming one (classic-agent \
                      retirement memo, internal-docs/2026-11-09-classic-agent-retirement.md, \
                      phase 2). Each failure names the file and points at `oxy \
                      migrate-agents --dry-run --json`. The previous revision keeps serving, \
                      so a workspace that still holds a classic file stops picking up \
                      changes until it migrates: run the gate census (memo 5.2) first, then \
                      turn this on in dev, staging and production in that order. A new \
                      flag, so it has no environment override.",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        // Read by the compile, which has no org context beyond the workspace.
        per_org: false,
        client: false,
        kind: FlagKind::Rollout,
        sunset: Some("2027-03-31"),
    },
];

pub fn get(key: &str) -> Option<&'static FlagDef> {
    FLAGS.iter().find(|f| f.key == key)
}

pub fn default_for(key: &str) -> bool {
    get(key).map(|f| f.default_enabled).unwrap_or(false)
}

/// A rollout gate's planned end, as [`FlagDef::sunset`] spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sunset {
    Date(chrono::NaiveDate),
    Version((u64, u64, u64)),
}

impl Sunset {
    /// `YYYY-MM-DD`, or `MAJOR.MINOR.PATCH`; anything else is `None`.
    pub fn parse(raw: &str) -> Option<Self> {
        if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
            return Some(Sunset::Date(date));
        }
        let mut parts = raw.split('.').map(|p| p.parse::<u64>().ok());
        match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(Some(a)), Some(Some(b)), Some(Some(c)), None) => Some(Sunset::Version((a, b, c))),
            _ => None,
        }
    }

    /// Whether the gate should already be gone, given today and this build's
    /// version: a date within [`SUNSET_MARGIN_DAYS`] of today, or a version this
    /// build has reached.
    pub fn is_due(&self, today: chrono::NaiveDate, version: (u64, u64, u64)) -> bool {
        match self {
            Sunset::Date(date) => today + chrono::Duration::days(SUNSET_MARGIN_DAYS) >= *date,
            Sunset::Version(v) => version >= *v,
        }
    }
}

impl FlagDef {
    /// The off/control variant.
    pub fn off(&self) -> &'static str {
        self.variants[0]
    }

    /// The variant a flag that is on, with no rules, serves.
    pub fn on(&self) -> &'static str {
        self.variants[1]
    }

    /// The registry's own spelling of `name`, if the flag declares it.
    pub fn variant(&self, name: &str) -> Option<&'static str> {
        self.variants.iter().copied().find(|v| *v == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn every_flag_declares_an_off_and_an_on_variant_once() {
        for flag in FLAGS {
            assert!(flag.variants.len() >= 2, "{} needs two variants", flag.key);
            let unique: HashSet<_> = flag.variants.iter().collect();
            assert_eq!(
                unique.len(),
                flag.variants.len(),
                "{} repeats a variant",
                flag.key
            );
        }
    }

    #[test]
    fn react_runtime_flag_is_registered_per_org_and_off() {
        let flag = get(super::super::REACT_RUNTIME).expect("react_runtime is declared");
        assert_eq!(flag.key, "react_runtime");
        assert!(flag.per_org, "read with the workspace's org");
        assert!(
            !flag.default_enabled,
            "the pilot is off until an org is opted in"
        );
        assert!(!flag.client, "server-only");
    }

    #[test]
    fn react_memory_flag_is_registered_per_org_and_off() {
        let flag = get(super::super::REACT_MEMORY).expect("react_memory is declared");
        assert!(flag.per_org, "read with the workspace's org");
        assert!(!flag.default_enabled, "off until an org is opted in");
        assert!(!flag.client, "server-only");
    }

    #[test]
    fn keys_are_unique() {
        let unique: HashSet<_> = FLAGS.iter().map(|f| f.key).collect();
        assert_eq!(unique.len(), FLAGS.len());
    }

    fn crate_version() -> (u64, u64, u64) {
        let mut parts = env!("CARGO_PKG_VERSION")
            .split('.')
            .map(|p| p.parse().unwrap());
        (
            parts.next().unwrap(),
            parts.next().unwrap(),
            parts.next().unwrap(),
        )
    }

    /// A rollout gate that has reached its sunset fails the build, naming the
    /// flag and where the retirement is described.
    #[test]
    fn every_rollout_flag_has_a_sunset_that_has_not_come() {
        let today = chrono::Utc::now().date_naive();
        for flag in FLAGS {
            match flag.kind {
                FlagKind::Product => assert!(
                    flag.sunset.is_none(),
                    "{}: a Product flag has no sunset; make it Rollout if it is one",
                    flag.key
                ),
                FlagKind::Rollout => {
                    let raw = flag.sunset.unwrap_or_else(|| {
                        panic!(
                            "{}: a Rollout flag must name a sunset (a date or a version)",
                            flag.key
                        )
                    });
                    let sunset = Sunset::parse(raw).unwrap_or_else(|| {
                        panic!(
                            "{}: sunset {raw:?} is neither YYYY-MM-DD nor MAJOR.MINOR.PATCH",
                            flag.key
                        )
                    });
                    assert!(
                        !sunset.is_due(today, crate_version()),
                        "the rollout gate `{}` reaches its sunset ({raw}) within {SUNSET_MARGIN_DAYS} days \
                         of today ({today}, build {}). Retire it: turn the change on, delete the FlagDef \
                         and its env override, and update the docs — internal-docs/feature-flags.md, \
                         \"Rollout gates and their sunset\".",
                        flag.key,
                        env!("CARGO_PKG_VERSION")
                    );
                }
            }
        }
    }

    #[test]
    fn a_sunset_is_due_within_the_margin_or_at_the_version() {
        let date = Sunset::parse("2027-01-31").unwrap();
        let day = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        assert!(!date.is_due(day("2026-12-01"), (0, 0, 0)));
        assert!(
            date.is_due(day("2027-01-20"), (0, 0, 0)),
            "inside the margin"
        );
        assert!(date.is_due(day("2027-03-01"), (0, 0, 0)));
        let version = Sunset::parse("0.6.0").unwrap();
        assert!(!version.is_due(day("2099-01-01"), (0, 5, 200)));
        assert!(version.is_due(day("2000-01-01"), (0, 6, 0)));
        assert_eq!(Sunset::parse("soon"), None);
    }
}
