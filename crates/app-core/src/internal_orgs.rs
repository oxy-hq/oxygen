//! The deployment's **internal orgs**: the orgs Oxy operates for itself, listed
//! in [`INTERNAL_ORGS_ENV`] as comma-separated org UUIDs. Unset or empty means
//! the deployment has none.
//!
//! Configuration, not an `oxy-authz` ring: what being internal grants is
//! decided by each consumer, and whoever can publish into, or open apps in, an
//! internal org inherits it. The first consumer is `ctx.platform.facts()`
//! (`oxy-app`, `custom_apps_functions/host/platform_facts`): an app in an
//! internal org may read ids and counts about every org, members of an internal
//! org count as staff there, and the facts mark these orgs `internal`.

use std::sync::OnceLock;

use uuid::Uuid;

/// The environment variable holding the list.
pub const INTERNAL_ORGS_ENV: &str = "OXY_INTERNAL_ORGS";

/// The org ids in `raw` (the value of [`INTERNAL_ORGS_ENV`]). An entry that is
/// not a UUID is logged and skipped rather than failing the list: one typo must
/// not drop every listed org. Duplicates collapse.
pub fn parse_internal_orgs(raw: Option<&str>) -> Vec<Uuid> {
    let mut orgs = Vec::new();
    let entries = raw.unwrap_or_default().split(',').map(str::trim);
    for entry in entries.filter(|e| !e.is_empty()) {
        match Uuid::parse_str(entry) {
            Ok(id) if orgs.contains(&id) => {}
            Ok(id) => orgs.push(id),
            Err(_) => tracing::warn!(
                entry,
                "{INTERNAL_ORGS_ENV}: skipping an entry that is not an org UUID"
            ),
        }
    }
    orgs
}

/// This process's internal orgs, taken from the boot config on the first call
/// and kept: the environment is fixed for a process's life, so a typo warns
/// once rather than on every caller's call.
pub fn internal_orgs() -> &'static [Uuid] {
    static INTERNAL: OnceLock<Vec<Uuid>> = OnceLock::new();
    INTERNAL.get_or_init(|| crate::config::BootConfig::current().internal_orgs.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORG_A: &str = "0f8e2b9c-3a51-4d6e-9c1f-2b7a4e5d6c01";
    const ORG_B: &str = "7c2d1e4f-5a6b-4c3d-8e9f-0a1b2c3d4e02";

    fn uuid(s: &str) -> Uuid {
        Uuid::parse_str(s).unwrap()
    }

    #[test]
    fn the_list_keeps_the_uuids_and_skips_junk() {
        let raw = format!(" {ORG_A}, junk ,{ORG_B}");
        assert_eq!(
            parse_internal_orgs(Some(&raw)),
            vec![uuid(ORG_A), uuid(ORG_B)]
        );
    }

    #[test]
    fn an_unset_list_is_empty() {
        assert!(parse_internal_orgs(None).is_empty());
        assert!(parse_internal_orgs(Some("")).is_empty());
        assert!(parse_internal_orgs(Some(" , ,")).is_empty());
    }

    #[test]
    fn a_repeated_org_is_listed_once() {
        let raw = format!("{ORG_A},{ORG_A}");
        assert_eq!(parse_internal_orgs(Some(&raw)), vec![uuid(ORG_A)]);
    }

    /// Read once per process: a change to the variable after the first call
    /// changes nothing, so a typo in it is parsed, and warned about, once.
    #[test]
    fn the_list_is_parsed_once_per_process() {
        // nextest runs each test in a process of its own, so this env is ours.
        unsafe { std::env::set_var(INTERNAL_ORGS_ENV, format!("{ORG_A}, not-a-uuid")) };
        assert_eq!(internal_orgs().to_vec(), vec![uuid(ORG_A)]);
        unsafe { std::env::set_var(INTERNAL_ORGS_ENV, ORG_B) };
        assert_eq!(
            internal_orgs().to_vec(),
            vec![uuid(ORG_A)],
            "the second call reads the first parse, not the variable"
        );
    }
}
