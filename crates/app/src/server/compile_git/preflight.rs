//! What a fetched tree must hold before it is worth compiling.
//!
//! Two checks, both about a compile that would otherwise *succeed* and promote
//! something broken.
//!
//! **A `config.yml` at the workspace root.** `workspaces.repo_subdir` is NULL
//! for the repository root and also for a row nobody has backfilled, so a
//! workspace that lives in `analytics/` can arrive here pointing at the root.
//! The compiler would walk the whole repository and promote it.
//!
//! **Every local-file DuckDB database is in the commit.** The compiler mirrors
//! a local DuckDB dataset to S3 and records where (`s3_mirror`), which is the
//! only way a pod with no working copy can query it. That step is best-effort:
//! a dataset that is not on disk is skipped without a word. On the node with
//! the working copy the data is there and the worst case is a workspace only
//! that node can serve. Here the tree is deleted when the compile ends, so a
//! skipped mirror is a database that points at nothing — on every node.
//!
//! So a commit compile refuses the tree instead, naming the database. Carrying
//! the previous revision's mirror forward would be the gentler rule, but the
//! mirror is written inside `oxy-compile`, which this path deliberately leaves
//! untouched (`internal-docs/factory-retirement.md`, phase 1).

use std::path::{Component, Path};

use serde_yaml::Value;

use super::CompileGitError;

/// Check the tree rooted at `workspace_root`. `mirror_configured` is whether
/// the compile has an S3 bucket to mirror into
/// (`oxy_compile::blob_store::bucket()`); it is a parameter so the rule can be
/// tested without the environment.
pub(super) fn check(workspace_root: &Path, mirror_configured: bool) -> Result<(), CompileGitError> {
    let config = workspace_root.join("config.yml");
    let Ok(text) = std::fs::read_to_string(&config) else {
        return Err(CompileGitError::NoConfig);
    };
    // A config.yml that does not parse is the compiler's to report: it does so
    // per file, on a revision that is recorded and never promoted.
    let Ok(parsed) = serde_yaml::from_str::<Value>(&text) else {
        return Ok(());
    };
    let databases = parsed.get("databases").and_then(Value::as_sequence);
    for database in databases.into_iter().flatten() {
        check_duckdb(workspace_root, database, mirror_configured)?;
    }
    Ok(())
}

/// Reads the same three keys, in the same order of preference, as
/// `oxy_compile::duckdb_mirror::mirror_duckdb_databases` — the code whose
/// silent skip this stands in front of.
fn check_duckdb(
    workspace_root: &Path,
    database: &Value,
    mirror_configured: bool,
) -> Result<(), CompileGitError> {
    let field = |key: &str| database.get(key).and_then(Value::as_str);
    if field("type") != Some("duckdb") {
        return Ok(());
    }
    let name = field("name").unwrap_or("duckdb").to_string();
    // The mirror joins this path onto the tree and uploads what it finds. On
    // a shared worker, `/proc/self/environ` or `../..` is the host's, and it
    // would land in this workspace's S3 mirror where its owner can read it.
    if let Some(outside) = [field("dataset"), field("path")]
        .into_iter()
        .flatten()
        .find(|p| is_local(p) && !stays_in_tree(p))
    {
        return Err(CompileGitError::DuckDbPathOutsideTree {
            database: name,
            path: outside.to_string(),
        });
    }
    let (path, present) = match (field("dataset"), field("path")) {
        (Some(dir), _) if is_local(dir) => (dir, has_data_file(&workspace_root.join(dir))),
        (_, Some(file)) if is_local(file) => (file, workspace_root.join(file).is_file()),
        // DuckLake, MotherDuck, an `s3://` path: nothing local to mirror.
        _ => return Ok(()),
    };
    if !present {
        return Err(CompileGitError::DuckDbDataMissing {
            database: name,
            path: path.to_string(),
        });
    }
    if !mirror_configured {
        return Err(CompileGitError::DuckDbMirrorUnconfigured { database: name });
    }
    Ok(())
}

fn is_local(path: &str) -> bool {
    !path.contains("://")
}

/// A relative path made only of ordinary names: nothing absolute, no `..`.
fn stays_in_tree(path: &str) -> bool {
    Path::new(path)
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

/// Whether `dir` holds at least one file the mirror would upload as a table.
fn has_data_file(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().map(|e| e.path()).any(|p| {
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or_default();
        p.is_file() && (ext.eq_ignore_ascii_case("csv") || ext.eq_ignore_ascii_case("parquet"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(config: &str, files: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.yml"), config).unwrap();
        for file in files {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"x").unwrap();
        }
        dir
    }

    const LOCAL_DATASET: &str = "databases:\n  - name: sales\n    type: duckdb\n    dataset: .db\n";
    const LOCAL_FILE: &str =
        "databases:\n  - name: ledger\n    type: duckdb\n    path: data/ledger.duckdb\n";

    #[test]
    fn a_tree_with_no_config_at_its_root_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            check(dir.path(), true),
            Err(CompileGitError::NoConfig)
        ));
    }

    #[test]
    fn a_committed_dataset_passes() {
        let dir = tree(LOCAL_DATASET, &[".db/orders.parquet"]);
        assert!(check(dir.path(), true).is_ok());
        let dir = tree(LOCAL_FILE, &["data/ledger.duckdb"]);
        assert!(check(dir.path(), true).is_ok());
    }

    /// The case the rule exists for: the data is on the Factory's disk and was
    /// never committed, so the commit has the config and not the files.
    #[test]
    fn a_dataset_that_is_not_in_the_commit_is_refused_by_name() {
        for (config, files, database, path) in [
            (LOCAL_DATASET, &[][..], "sales", ".db"),
            // The directory is there, with nothing the mirror would upload.
            (LOCAL_DATASET, &[".db/README.md"][..], "sales", ".db"),
            (LOCAL_FILE, &[][..], "ledger", "data/ledger.duckdb"),
        ] {
            let dir = tree(config, files);
            match check(dir.path(), true) {
                Err(CompileGitError::DuckDbDataMissing {
                    database: d,
                    path: p,
                }) => assert_eq!((d.as_str(), p.as_str()), (database, path)),
                other => panic!("expected DuckDbDataMissing for {database}, got {other:?}"),
            }
        }
    }

    /// A commit is the tenant's; the pod it compiles on is not. The mirror
    /// would upload whatever these paths name on the host.
    #[test]
    fn a_database_path_that_leaves_the_tree_is_refused_by_name() {
        for (key, outside) in [
            ("path", "/proc/self/environ"),
            ("path", "../../etc/passwd"),
            ("dataset", "/var/lib"),
            ("dataset", "data/../../.."),
        ] {
            let config =
                format!("databases:\n  - name: sales\n    type: duckdb\n    {key}: {outside}\n");
            let dir = tree(&config, &[]);
            match check(dir.path(), true) {
                Err(CompileGitError::DuckDbPathOutsideTree { database, path }) => {
                    assert_eq!((database.as_str(), path.as_str()), ("sales", outside));
                }
                other => panic!("expected DuckDbPathOutsideTree for {outside}, got {other:?}"),
            }
        }
    }

    #[test]
    fn committed_data_with_nowhere_to_mirror_it_is_refused_by_name() {
        let dir = tree(LOCAL_DATASET, &[".db/orders.csv"]);
        assert!(matches!(
            check(dir.path(), false),
            Err(CompileGitError::DuckDbMirrorUnconfigured { database }) if database == "sales"
        ));
    }

    #[test]
    fn databases_with_nothing_on_local_disk_are_not_this_rules_business() {
        let config = "databases:\n  \
            - name: lake\n    type: duckdb\n    dataset: s3://bucket/lake\n  \
            - name: wh\n    type: postgres\n    host: db.internal\n";
        let dir = tree(config, &[]);
        assert!(check(dir.path(), false).is_ok());
        assert!(check(tree("databases: []\n", &[]).path(), false).is_ok());
    }

    #[test]
    fn a_config_that_does_not_parse_is_left_for_the_compiler_to_report() {
        let dir = tree("databases: [unterminated\n", &[]);
        assert!(check(dir.path(), true).is_ok());
    }
}
