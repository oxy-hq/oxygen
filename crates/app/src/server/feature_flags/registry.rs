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
        key: super::APPS_FIRST_EXPERIENCE,
        description: "The apps-first experience, per org. When on for an org: (1) \
                      customers get the new home (their apps, an ask box, how their data \
                      fits together, recent questions) and the new onboarding screens; \
                      (2) the web app drops \"Manage sets of apps\", the set switcher, \
                      the git details on workspace cards and the web workspace-creation \
                      screens (Oxy staff use oxyc and the API); (3) customers can open \
                      a read-only world model page; (4) Factory access is enforced: every \
                      Factory route refuses everyone but Oxy staff, the org's owners and \
                      admins, and the members an operator granted that workspace to \
                      (403 factory_access_required), and the rail hides the Factory \
                      from the rest. When off, everything is as before and nobody is \
                      refused the Factory. Grant the plain members who use the Factory \
                      today BEFORE turning this on. Replaces factory_access_enforcement.",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        per_org: true,
        client: true,
        kind: FlagKind::Product,
        sunset: None,
    },
    FlagDef {
        key: super::HOME_V3,
        description: "Home page v3, for every org at once, and only where \
                      apps_first_experience is on. When on: the ask box comes first, \
                      the apps follow, bigger, and Today folds to one line under them; \
                      a Simple/Full switch (members start on Simple, owners and admins \
                      on Full; Full adds recent questions and the world model); the \
                      top bar loses its clock and gains a Today button. When off, the \
                      apps-first home is as before (greeting, Today beside the apps). \
                      Presentation only: nothing is refused either way.",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        // Read from `GET /auth/config`, which has no org.
        per_org: false,
        client: false,
        kind: FlagKind::Product,
        sunset: None,
    },
    FlagDef {
        key: crate::server::queued_analytics::QUEUED_STARTS_FLAG,
        description: "Start a plain chat and a member's agentic-workflow run on the \
                      queue, driven by whichever pod claims it, instead of in the pod \
                      that took the request. ON BY DEFAULT since every pod runs a build \
                      with the queued chat executor; this is now the kill switch. Turn \
                      off to drive both in the pod that took the request, as before the \
                      queue (a run already queued is still finished by a driver). The \
                      switch is kept for one release and then removed.",
        default_enabled: true,
        variants: BOOLEAN,
        unit: Unit::Org,
        per_org: false,
        client: false,
        kind: FlagKind::Product,
        sunset: None,
    },
    FlagDef {
        key: super::rollout::COMPILE_RECONCILE_FLAG,
        description: "Keep a remote-backed workspace serving what its default branch has: \
                      the periodic branch check (every 5 minutes per workspace) and the \
                      GitHub push webhook queue a commit compile when the branch is ahead \
                      of what is served. Turning it on changes what the deployment serves \
                      without anyone clicking Compile; the webhook also needs \
                      GITHUB_WEBHOOK_SECRET. Was OXY_COMPILE_RECONCILE, which still \
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
    FlagDef {
        key: super::rollout::IDE_RETIRED_FLAG,
        description: "Retire the in-product IDE on this deployment, before its code is \
                      deleted (factory-retirement.md, phase 5). When on: the web app hides \
                      Files, the Database client, the git half of the IDE header and chat's \
                      Build mode, and shows the workspace's repository (clone, open in VS \
                      Code, oxyc workspace clone) where Files was; the server answers every \
                      route that edits files, runs git or compiles the working copy with \
                      410 ide_retired (naming the repository), and a ?branch= request is no \
                      longer sent to the Factory. The rest of the back office under /ide, \
                      chat, runs and data flow are untouched. When off, everything is as \
                      before. When it is on everywhere, the removal PR deletes the IDE and \
                      this flag.",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        // Read from `GET /auth/config` and per request, neither with an org.
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
