//! Every `OXY_*` environment read goes through `oxy_app_core::config`, or is in
//! the backlog ([`allowed::ALLOWED`]) with a reason.
//!
//! Before this guard, 183 variables were read at 214 sites across the crates,
//! each with its default encoded where it happened to be read. The config
//! module (`crates/app-core/src/config/`) is the one place a deployment
//! variable is parsed, defaulted and validated; `internal-docs/configuration.md`
//! is the inventory, generated from the same scan by
//! `scripts/env-inventory.py` and never hand-edited.
//!
//! Three checks, one subject:
//!
//! * a read outside the config module that is not in the backlog fails, and a
//!   backlog entry whose site is gone fails too, so the list only shrinks;
//! * the Python scan and this one find the same sites, so the doc cannot drift
//!   from what the guard enforces;
//! * the doc is current: regenerate-and-diff, as the roadmap summary is checked.
//!
//! No database, no router, no process-global: this group's rules hold.

mod allowed;
mod scan;

use std::collections::BTreeSet;
use std::process::Command;

use allowed::{ALLOWED, ALLOWED_PENDING_PREFIXES};
use scan::{CENTRAL, Site, repo_root, scan};

fn is_pending(file: &str) -> bool {
    ALLOWED_PENDING_PREFIXES
        .iter()
        .any(|(prefix, _)| file.starts_with(prefix))
}

#[test]
fn every_oxy_env_read_is_central_or_in_the_backlog() {
    let root = repo_root();
    let sites = scan(&root);
    let backlog: BTreeSet<Site> = ALLOWED
        .iter()
        .map(|(var, file, _)| (var.to_string(), file.to_string()))
        .collect();

    let new_reads: Vec<&Site> = sites
        .iter()
        .filter(|(_, file)| !file.starts_with(CENTRAL) && !is_pending(file))
        .filter(|site| !backlog.contains(*site))
        .collect();
    assert!(
        new_reads.is_empty(),
        "{} `OXY_*` read(s) outside `oxy_app_core::config` and not in the backlog:\n{}\n\n\
         Read the variable through `oxy_app_core::config::BootConfig` (parsed once at boot, \
         default and validation in one place) instead of `std::env::var` at the call site. \
         Then add the variable to scripts/env-inventory.json and regenerate \
         internal-docs/configuration.md with `python3 -I scripts/env-inventory.py`.",
        new_reads.len(),
        new_reads
            .iter()
            .map(|(var, file)| format!("  {var} in {file}"))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let stale: Vec<&Site> = backlog
        .iter()
        .filter(|site| !sites.contains(*site))
        .collect();
    assert!(
        stale.is_empty(),
        "{} backlog entr{} in env_reads/allowed.rs name a read that no longer exists; \
         delete {} so the backlog only shrinks:\n{}",
        stale.len(),
        if stale.len() == 1 { "y" } else { "ies" },
        if stale.len() == 1 { "it" } else { "them" },
        stale
            .iter()
            .map(|(var, file)| format!("  {var} in {file}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_backlog_has_no_duplicate_entries() {
    let mut seen = BTreeSet::new();
    for (var, file, _) in ALLOWED {
        assert!(seen.insert((var, file)), "{var} in {file} is listed twice");
    }
}

fn inventory(args: &[&str]) -> std::process::Output {
    let root = repo_root();
    Command::new("python3")
        .arg("-I")
        .arg(root.join("scripts/env-inventory.py"))
        .arg("--root")
        .arg(&root)
        .args(args)
        .output()
        .expect(
            "python3 is needed to check internal-docs/configuration.md against the sources; \
             it is on every CI runner and every developer machine",
        )
}

/// The script's scan and [`scan`] must agree, or the doc describes one set of
/// sites and the guard enforces another.
#[test]
fn inventory_script_agrees_with_this_scan() {
    let out = inventory(&["--sites"]);
    assert!(
        out.status.success(),
        "scripts/env-inventory.py --sites failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let script: BTreeSet<Site> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(var, file)| (var.to_string(), file.to_string()))
        .collect();
    let here = scan(&repo_root());
    let only_script: Vec<_> = script.difference(&here).collect();
    let only_here: Vec<_> = here.difference(&script).collect();
    assert!(
        only_script.is_empty() && only_here.is_empty(),
        "the two scans disagree (a rule changed in one and not the other).\n\
         Only the script finds: {only_script:?}\nOnly this test finds: {only_here:?}"
    );
}

/// `internal-docs/configuration.md` is generated: regenerate it and diff.
#[test]
fn configuration_doc_is_current() {
    let out = inventory(&["--check"]);
    assert!(
        out.status.success(),
        "internal-docs/configuration.md is not what the sources say (exit {:?}):\n{}\n\
         Run `python3 -I scripts/env-inventory.py` and commit the result; a variable \
         with no entry in scripts/env-inventory.json needs one (kind, default, gates).",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    );
}
