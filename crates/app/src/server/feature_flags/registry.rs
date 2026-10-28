//! Code-declared feature flag registry.
//!
//! Adding a flag: append a `FlagDef` entry below. Removing a flag: delete
//! the entry; any DB row with that key becomes stale and is filtered out
//! at cache load (see `cache::load_map`). Leave that row in place: a rollback
//! to a build that still declares the flag reads its value back, where a
//! deleted row would read the default. Renaming: treat as remove + add.

pub struct FlagDef {
    pub key: &'static str,
    pub description: &'static str,
    pub default_enabled: bool,
}

pub static FLAGS: &[FlagDef] = &[
    FlagDef {
        key: "billing",
        description: "Master switch for billing & subscription enforcement. \
                      When off, paywall and subscription guards are skipped \
                      for all orgs.",
        default_enabled: false,
    },
    FlagDef {
        key: super::APPS_FIRST_EXPERIENCE,
        description: "The apps-first experience, for every org at once. When on: (1) \
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
    },
];

pub fn get(key: &str) -> Option<&'static FlagDef> {
    FLAGS.iter().find(|f| f.key == key)
}

pub fn default_for(key: &str) -> bool {
    get(key).map(|f| f.default_enabled).unwrap_or(false)
}
