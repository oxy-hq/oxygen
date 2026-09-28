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

pub static FLAGS: &[FlagDef] = &[FlagDef {
    key: "billing",
    description: "Master switch for billing & subscription enforcement. \
                      When off, paywall and subscription guards are skipped \
                      for all orgs.",
    default_enabled: false,
}];

pub fn get(key: &str) -> Option<&'static FlagDef> {
    FLAGS.iter().find(|f| f.key == key)
}

pub fn default_for(key: &str) -> bool {
    get(key).map(|f| f.default_enabled).unwrap_or(false)
}
