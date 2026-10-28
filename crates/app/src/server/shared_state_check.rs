//! Boot-time refusal: a process that is not the single process must not fall
//! back to this node's disk for state other nodes have to read.
//!
//! Three things have a local-disk fallback that is right for one process and
//! wrong for a fleet:
//!
//! - **The encryption key.** Without `OXY_ENCRYPTION_KEY` the loader reads, or
//!   invents and saves, `<state dir>/encryption_key.txt`. A second node invents
//!   its own key, so a secret one node sealed is unreadable on the next (the
//!   2026-08 diskless-replica findings: one secret answered 200 on one replica
//!   and 404 on the others).
//! - **The custom-app build store** (`OXY_CUSTOMER_APPS_S3_BUCKET`): bundles
//!   and function artifacts go to this node's disk, so an asset request that
//!   lands on another replica 404s.
//! - **The custom-app asset store** (`OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET`,
//!   `ctx.storage` and documents): files land on this node's disk and
//!   silently vanish from every other node's point of view.
//!
//! `OXY_ROLE=all` (what `oxy serve` is without an `OXY_ROLE`, and what
//! `oxy start` runs) is the single process, and keeps every fallback. Every
//! other role (`ide`, `serve`, `worker`) refuses to start, naming each setting
//! it is missing, instead of booting into a state that fails later and
//! somewhere else. Such a process also may never create the key file later
//! (`oxy::utils::forbid_key_file_creation`).
//!
//! A key that is set is not yet the right key. [`check_encryption_key`] runs
//! after migrations, in every role including `all`, and refuses a key that
//! does not open this database's encryption check value
//! (`oxy_platform::secrets::key_check`, which also has the rule for a
//! deployment that has sealed data and no check value yet).

use oxy_app_core::process_role::Role;

/// Why a setting is required, for the error message.
struct Requirement {
    var: &'static str,
    why: &'static str,
}

/// The settings every role other than `all` must have, in the order they are
/// reported. The names come from the code that reads them, so a rename there
/// cannot leave this check naming a variable nothing reads.
fn requirements() -> [Requirement; 3] {
    [
        Requirement {
            var: oxy::utils::ENCRYPTION_KEY_VAR,
            why: "the AES key for stored secrets; base64 of 32 bytes, e.g. `openssl rand -base64 32`. \
                  Every node must hold the same value",
        },
        Requirement {
            var: oxy_custom_apps::build_store::BUCKET_VAR,
            why: "S3 bucket for custom-app bundles and function artifacts",
        },
        Requirement {
            var: oxy_custom_apps::storage::BUCKET_VAR,
            why: "S3 bucket for custom-app `ctx.storage` files and org documents",
        },
    ]
}

/// `Err(message)` when `role` is not the single process and a required
/// setting is missing or, for the key, unusable. `read` is the environment, so
/// the check is testable without touching the process's.
pub fn check(role: Role, read: impl Fn(&str) -> Option<String>) -> Result<(), String> {
    if role == Role::All {
        return Ok(());
    }
    let mut problems = Vec::new();
    for req in requirements() {
        let value = read(req.var).filter(|v| !v.trim().is_empty());
        match value {
            None => problems.push(format!("  {}: not set. {}", req.var, req.why)),
            Some(v) if req.var == oxy::utils::ENCRYPTION_KEY_VAR => {
                if let Err(e) = oxy::utils::parse_encryption_key(&v) {
                    problems.push(format!(
                        "  {}: set but unusable ({e}). {}",
                        req.var, req.why
                    ));
                }
            }
            Some(_) => {}
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "OXY_ROLE={} needs state that every node can read, and these settings are missing:\n{}\n\n\
         Without them this process would keep the state on its own disk, where no other node \
         can see it. A single process (OXY_ROLE=all, or `oxy start`) does not need them.",
        role.as_str(),
        problems.join("\n"),
    ))
}

/// [`check`] against the real environment, for the entry points. A process
/// that is not the single process is also barred from creating a key file of
/// its own for the rest of its life.
pub fn check_process_env(role: Role) -> Result<(), String> {
    if role != Role::All {
        oxy::utils::forbid_key_file_creation(format!("OXY_ROLE={}", role.as_str()));
    }
    check(role, |name| std::env::var(name).ok())
}

/// The second half, for the entry points after migrations: the key this
/// process loaded must open what this database's values were sealed with.
/// `Err(message)` names the key's source and what an operator does next.
pub async fn check_encryption_key() -> Result<(), String> {
    let db = oxy::database::client::establish_connection()
        .await
        .map_err(|e| format!("Could not check the encryption key against the database: {e}"))?;
    oxy::adapters::secrets::key_check::check_at_boot(&db).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const KEY: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="; // 32 zero bytes

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    fn full() -> Vec<(&'static str, &'static str)> {
        vec![
            ("OXY_ENCRYPTION_KEY", KEY),
            ("OXY_CUSTOMER_APPS_S3_BUCKET", "builds"),
            ("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET", "assets"),
        ]
    }

    #[test]
    fn the_names_checked_are_the_ones_read() {
        // The check takes its names from the reading code; pin the spellings
        // an operator types so a rename shows up here and in the runbooks.
        let names: Vec<&str> = requirements().iter().map(|r| r.var).collect();
        assert_eq!(
            names,
            [
                "OXY_ENCRYPTION_KEY",
                "OXY_CUSTOMER_APPS_S3_BUCKET",
                "OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET"
            ]
        );
    }

    #[test]
    fn the_single_process_needs_none_of_it() {
        assert_eq!(check(Role::All, env(&[])), Ok(()));
    }

    #[test]
    fn every_other_role_refuses_an_empty_environment_naming_each_setting() {
        for role in [Role::Ide, Role::Serve, Role::Worker] {
            let err = check(role, env(&[])).expect_err("must refuse");
            assert!(
                err.starts_with(&format!("OXY_ROLE={} needs", role.as_str())),
                "{err}"
            );
            for var in [
                "OXY_ENCRYPTION_KEY",
                "OXY_CUSTOMER_APPS_S3_BUCKET",
                "OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET",
            ] {
                assert!(err.contains(var), "{role:?}: {var} not named in {err}");
            }
        }
    }

    #[test]
    fn a_complete_environment_passes_in_every_role() {
        for role in [Role::Ide, Role::Serve, Role::Worker, Role::All] {
            assert_eq!(check(role, env(&full())), Ok(()), "{role:?}");
        }
    }

    #[test]
    fn only_the_missing_settings_are_named() {
        let mut pairs = full();
        pairs.retain(|(k, _)| *k != "OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET");
        let err = check(Role::Serve, env(&pairs)).expect_err("must refuse");
        assert!(err.contains("OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET"), "{err}");
        assert!(!err.contains("OXY_ENCRYPTION_KEY"), "{err}");
        assert!(!err.contains("OXY_CUSTOMER_APPS_S3_BUCKET:"), "{err}");
    }

    #[test]
    fn a_blank_value_is_not_a_value() {
        let mut pairs = full();
        pairs.retain(|(k, _)| *k != "OXY_CUSTOMER_APPS_S3_BUCKET");
        pairs.push(("OXY_CUSTOMER_APPS_S3_BUCKET", "   "));
        let err = check(Role::Worker, env(&pairs)).expect_err("must refuse");
        assert!(
            err.contains("OXY_CUSTOMER_APPS_S3_BUCKET: not set"),
            "{err}"
        );
    }

    /// The loader panics on a malformed key at the first secret read. The
    /// check says so at boot, as an error that names the variable.
    #[test]
    fn an_unusable_key_is_refused_with_the_reason() {
        for (bad, reason) in [("not base64!!", "base64"), ("QUJD", "32 bytes")] {
            let mut pairs = full();
            pairs.retain(|(k, _)| *k != "OXY_ENCRYPTION_KEY");
            pairs.push(("OXY_ENCRYPTION_KEY", bad));
            let err = check(Role::Ide, env(&pairs)).expect_err("must refuse");
            assert!(
                err.contains("OXY_ENCRYPTION_KEY: set but unusable"),
                "{err}"
            );
            assert!(err.contains(reason), "{reason} not in {err}");
        }
    }
}
