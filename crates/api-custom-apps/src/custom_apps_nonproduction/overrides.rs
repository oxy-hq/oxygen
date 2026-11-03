//! An environment's warehouse overrides (`planes.warehouse.destinations`),
//! checked when the choice is stored: the same checks a publish runs on the
//! build's `nonProduction.destinations` ([`check_destinations`]), so a mapped
//! database that is production's under another name is refused when someone
//! sets it, not on the first write. The host repeats them on every write, on
//! the map it runs with: the build's mapping under the overrides, so that is
//! the map the chain check is made on here too.

use std::collections::BTreeMap;

use uuid::Uuid;

use super::check_entries;

/// Why `overrides` cannot be stored for an environment of the app in
/// `project_id`, or `None`. Each override is checked as a publish checks a
/// mapping, against `merged`: the map the environment runs with, the build's
/// mapping under `overrides` (`EnvPolicy::destinations`). The chain check
/// also covers the build's own pairs in `merged`, which an override can turn
/// into a chain. Both a refusal and a
/// workspace that cannot be checked yet (nothing compiled) are reasons: an
/// override nobody could check is not stored. Reads nothing when there are no
/// overrides.
pub async fn check_overrides(
    project_id: Uuid,
    merged: &BTreeMap<String, String>,
    overrides: &BTreeMap<String, String>,
) -> Option<String> {
    match check_entries(project_id, merged, overrides).await {
        Ok(()) => None,
        Err(refusal) => Some(format!(
            "planes.warehouse.destinations is refused by the checks a publish runs on \
             nonProduction.destinations: {refusal}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nothing to check reads nothing: no workspace, no secrets, no database.
    #[tokio::test]
    async fn no_overrides_are_never_refused() {
        assert_eq!(
            check_overrides(Uuid::nil(), &BTreeMap::new(), &BTreeMap::new()).await,
            None
        );
    }
}
