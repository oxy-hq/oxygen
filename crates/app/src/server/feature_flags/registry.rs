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
    },
    FlagDef {
        key: crate::server::queued_analytics::QUEUED_STARTS_FLAG,
        description: "Start a plain chat and a member's agentic-workflow run on the \
                      queue, driven by whichever pod claims it, instead of in the pod \
                      that took the request. When off, both are driven where they \
                      are started, as before. Turn on only once every pod runs a build \
                      with the queued chat executor: an older pod claims a queued chat \
                      and fails it as an unknown kind.",
        default_enabled: false,
        variants: BOOLEAN,
        unit: Unit::Org,
        per_org: false,
        client: false,
    },
];

pub fn get(key: &str) -> Option<&'static FlagDef> {
    FLAGS.iter().find(|f| f.key == key)
}

pub fn default_for(key: &str) -> bool {
    get(key).map(|f| f.default_enabled).unwrap_or(false)
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
    use super::FLAGS;
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
}
