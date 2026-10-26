//! Decide which workspace packages a change has to test.
//!
//! The engine is guppy's [`determinator`]: it maps each changed path to the package
//! that owns it, diffs the old and new dependency graphs (versions, features, the
//! lockfile), and closes the result over reverse dependencies. A changed path that
//! no package and no rule claims marks the whole workspace, so an unknown input
//! costs a full run, never a skipped test.
//!
//! Two things the graph cannot see are configured in `.config/test-selection.toml`:
//! files one crate reads from another crate's directory or from outside `crates/`
//! (`[[determinator.path-rule]]`), and packages whose tests scan the whole tree
//! (`always-test`).

use std::collections::BTreeSet;

use determinator::Determinator;
use determinator::rules::DeterminatorRules;
use guppy::graph::cargo::CargoResolverVersion;
use guppy::graph::feature::StandardFeatures;
use guppy::graph::{DependencyDirection, PackageGraph};
use serde::{Deserialize, Serialize};

/// The hakari-managed crate. Determinator simulates each package's build with
/// it unified in, which is how Cargo actually builds this workspace.
const WORKSPACE_HACK: &str = "workspace-hack";

/// `.config/test-selection.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    /// Packages tested whenever anything is: their tests read sources across the
    /// whole tree, so no dependency edge says when they are affected.
    #[serde(default)]
    pub always_test: Vec<String>,
    /// Determinator path and package rules, in determinator's own format.
    #[serde(default)]
    pub determinator: DeterminatorRules,
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| format!("test-selection config: {e}"))
    }
}

/// What the tests have to cover.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "mode", content = "packages", rename_all = "kebab-case")]
pub enum Selection {
    /// Every workspace package (`--workspace`).
    All,
    /// These packages only (`-p` each). Never empty.
    Packages(BTreeSet<String>),
    /// Nothing that any test can observe changed.
    None,
}

/// The inputs `select` needs besides the config.
pub struct Change<'a> {
    pub old: &'a PackageGraph,
    pub new: &'a PackageGraph,
    pub changed_paths: &'a [String],
    pub resolver: CargoResolverVersion,
}

/// Runs the determinator and applies `always-test`.
pub fn select(change: &Change<'_>, config: &Config) -> Result<Selection, String> {
    check_always_test(change.new, config)?;

    let mut cargo_options = Determinator::default_cargo_options();
    cargo_options.set_resolver(change.resolver);

    let mut determinator = Determinator::new(change.old, change.new);
    determinator
        .set_rules(&config.determinator)
        .map_err(|e| format!("determinator rules: {e}"))?;
    determinator.set_cargo_options(&cargo_options);
    if change.new.workspace().contains_name(WORKSPACE_HACK) {
        determinator
            .set_features_only([WORKSPACE_HACK], StandardFeatures::Default)
            .map_err(|e| format!("features-only {WORKSPACE_HACK}: {e}"))?;
    }
    determinator.add_changed_paths(change.changed_paths.iter().map(String::as_str));

    let affected = determinator.compute().affected_set;
    if affected.len() == change.new.workspace().member_count() {
        return Ok(Selection::All);
    }
    let mut packages: BTreeSet<String> = affected
        .packages(DependencyDirection::Forward)
        .map(|p| p.name().to_owned())
        .collect();
    if packages.is_empty() {
        return Ok(Selection::None);
    }
    packages.extend(config.always_test.iter().cloned());
    Ok(Selection::Packages(packages))
}

/// A renamed or deleted package must fail the run, not silently drop its tests.
fn check_always_test(graph: &PackageGraph, config: &Config) -> Result<(), String> {
    let workspace = graph.workspace();
    let missing: Vec<&str> = config
        .always_test
        .iter()
        .map(String::as_str)
        .filter(|name| !workspace.contains_name(name))
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "always-test names packages that are not in the workspace: {}",
            missing.join(", ")
        ))
    }
}

/// One line per changed path saying what it pulled in, for the CI log.
pub fn explain(change: &Change<'_>, config: &Config) -> Result<Vec<String>, String> {
    let mut determinator = Determinator::new(change.old, change.new);
    determinator
        .set_rules(&config.determinator)
        .map_err(|e| format!("determinator rules: {e}"))?;
    let lines = change
        .changed_paths
        .iter()
        .map(|path| {
            let mut owners = Vec::new();
            let matched = determinator.match_path(path, |id| {
                if let Ok(package) = change.new.metadata(id) {
                    owners.push(package.name().to_owned());
                }
            });
            format!("{path}: {matched:?} {owners:?}")
        })
        .collect();
    Ok(lines)
}

/// The workspace's `resolver`, read from the root manifest so a bump to "3" is
/// picked up rather than simulated with the wrong feature unification.
pub fn workspace_resolver(root_manifest: &str) -> Result<CargoResolverVersion, String> {
    #[derive(Deserialize)]
    struct Manifest {
        workspace: Workspace,
    }
    #[derive(Deserialize)]
    struct Workspace {
        resolver: CargoResolverVersion,
    }
    toml::from_str::<Manifest>(root_manifest)
        .map(|m| m.workspace.resolver)
        .map_err(|e| format!("root Cargo.toml needs [workspace] resolver: {e}"))
}

#[cfg(test)]
mod tests;
