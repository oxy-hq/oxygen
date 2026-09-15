//! Bundle bytes → a plan, with no database in sight.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use super::types::{DeclaredMigration, MigrationError};
use crate::server::api::custom_apps_manifest::migrations_config;

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Normalise and vet the declared directory.
///
/// The bundle's paths are already `..`-free (`unpack_tar_gz` rejects traversal),
/// so this guards the *manifest* side: `dir` is author-supplied and is used as a
/// prefix match, where `"/"` or `""` would sweep the entire bundle into the
/// migration set and `".."` would read as a path the author did not intend.
fn normalize_dir(dir: &str) -> Result<String, MigrationError> {
    let trimmed = dir.trim().trim_matches('/');
    if trimmed.is_empty()
        || trimmed
            .split('/')
            .any(|c| c == ".." || c == "." || c.is_empty())
        || dir.starts_with('/')
        || dir.contains('\\')
    {
        return Err(MigrationError::UnsafeDir(dir.to_string()));
    }
    Ok(trimmed.to_string())
}

/// Pull the bundle's `*.sql` files under `dir`, in **lexical filename order**.
///
/// Pure — no database, no manifest — so the ordering and the checksum are
/// testable without a tenant. Ordering is by the path relative to `dir`, which
/// for the ordinary flat directory is filename order; a nested layout sorts
/// deterministically too rather than by whatever order tar happened to emit.
pub(crate) fn collect(
    files: &[(String, Vec<u8>)],
    dir: &str,
) -> Result<Vec<DeclaredMigration>, MigrationError> {
    let dir = normalize_dir(dir)?;
    let prefix = format!("{dir}/");
    let mut out = Vec::new();
    for (path, bytes) in files {
        let path = path.trim_start_matches("./");
        let Some(rel) = path.strip_prefix(&prefix) else {
            continue;
        };
        // Case-sensitive on purpose: object keys are, and accepting `.SQL`
        // here while the store treats it as a different file invites a bundle
        // that behaves differently on two machines.
        if !rel.ends_with(".sql") {
            continue;
        }
        let sql = String::from_utf8(bytes.clone()).map_err(|e| MigrationError::NotUtf8 {
            filename: rel.to_string(),
            message: e.to_string(),
        })?;
        out.push(DeclaredMigration {
            filename: rel.to_string(),
            checksum: sha256_hex(bytes),
            sql,
        });
    }
    if out.is_empty() {
        return Err(MigrationError::EmptyDir { dir });
    }
    out.sort_by(|a, b| a.filename.cmp(&b.filename));
    Ok(out)
}

/// Decide what still has to run, refusing anything that has already run.
///
/// `ledger` is `filename -> checksum` for THIS app. Returns the subset of
/// `declared` to apply, in the order given.
///
/// Three outcomes per file, and the two refusals are the point of the feature:
///
/// | ledger says | verdict |
/// | --- | --- |
/// | same name, same bytes | already applied — skip |
/// | same name, different bytes | [`MigrationError::ChecksumMismatch`] |
/// | different name, same bytes | [`MigrationError::AlreadyAppliedUnderAnotherName`] |
/// | nothing | apply |
///
/// A file in the ledger but absent from the bundle is **ignored**: it ran, and
/// deleting the file from the repo cannot un-run it.
pub(crate) fn plan<'a>(
    declared: &'a [DeclaredMigration],
    ledger: &HashMap<String, String>,
) -> Result<Vec<&'a DeclaredMigration>, MigrationError> {
    // Reverse index for the rename rule. Built once; a ledger is tens of rows.
    let by_checksum: HashMap<&str, &str> = ledger
        .iter()
        .map(|(name, sum)| (sum.as_str(), name.as_str()))
        .collect();

    let mut pending = Vec::new();
    for m in declared {
        match ledger.get(&m.filename) {
            Some(applied) if applied == &m.checksum => continue,
            Some(applied) => {
                return Err(MigrationError::ChecksumMismatch {
                    filename: m.filename.clone(),
                    applied: applied.clone(),
                    bundled: m.checksum.clone(),
                });
            }
            None => {}
        }
        // Not in the ledger by name — but if its *contents* already ran under a
        // different name, applying it would run that SQL a second time. For a
        // seed-style `INSERT ... ON CONFLICT` that is silent duplication rather
        // than a loud `already exists`, which is the measured bug.
        if let Some(applied_as) = by_checksum.get(m.checksum.as_str()) {
            return Err(MigrationError::AlreadyAppliedUnderAnotherName {
                filename: m.filename.clone(),
                applied_as: applied_as.to_string(),
            });
        }
        pending.push(m);
    }
    Ok(pending)
}

/// What the bundle declares: the manifest block resolved and its `*.sql` files
/// pulled out, or an empty vec when the manifest declares nothing.
///
/// Split from [`apply_on_promote`] so `publish` can call it **before** the
/// bundle is uploaded and before any row is written. Two things fall out of
/// that: a malformed `migrations` block fails the publish without leaving an
/// orphan build behind, and a *draft* publish validates the block too — the
/// author finds out the directory name is wrong on the publish that carries it,
/// not on the promote weeks later.
///
/// It also keeps the SQL alive past the point where `publish` moves the whole
/// bundle into the object store: these files are kilobytes, so holding the
/// subset costs nothing.
pub(crate) fn declare(
    manifest_json: Option<&serde_json::Value>,
    files: &[(String, Vec<u8>)],
) -> Result<Vec<DeclaredMigration>, MigrationError> {
    let Some(cfg) = migrations_config(manifest_json).map_err(MigrationError::BadManifest)? else {
        return Ok(Vec::new());
    };
    collect(files, &cfg.dir)
}

/// The bundle's `airhouseMigrations`, every file checked against the app's
/// schema and DuckLake's rules now — on a draft publish too — so a key or a
/// stray schema fails the publish that carries it, not a promote weeks later.
pub(crate) fn declare_airhouse(
    manifest_json: Option<&serde_json::Value>,
    files: &[(String, Vec<u8>)],
    app_slug: &str,
) -> Result<Vec<DeclaredMigration>, MigrationError> {
    let Some(cfg) =
        crate::server::api::custom_apps_manifest::airhouse_migrations_config(manifest_json)
            .map_err(MigrationError::BadManifest)?
    else {
        return Ok(Vec::new());
    };
    let declared = collect(files, &cfg.dir)?;
    let schema = super::airhouse::airhouse_schema_for(app_slug)?;
    for m in &declared {
        super::airhouse::check_file(m, &schema)?;
    }
    Ok(declared)
}

#[cfg(test)]
mod collect_tests {
    use super::*;

    fn bundle(entries: &[(&str, &str)]) -> Vec<(String, Vec<u8>)> {
        entries
            .iter()
            .map(|(p, c)| (p.to_string(), c.as_bytes().to_vec()))
            .collect()
    }

    /// Lexical order, and only `.sql` under the declared directory.
    #[test]
    fn collects_sql_under_the_dir_in_lexical_order() {
        let files = bundle(&[
            ("index.html", "<html>"),
            ("migrations/0002_seed.sql", "INSERT INTO t VALUES (1);"),
            ("migrations/0001_init.sql", "CREATE TABLE t (id int);"),
            ("migrations/README.md", "not sql"),
            ("other/0003_nope.sql", "CREATE TABLE u (id int);"),
        ]);
        let got = collect(&files, "migrations").expect("collect");
        assert_eq!(
            got.iter().map(|m| m.filename.as_str()).collect::<Vec<_>>(),
            vec!["0001_init.sql", "0002_seed.sql"],
            "tar order must not decide apply order"
        );
    }

    /// The ledger key is relative to the directory, so moving the directory
    /// does not re-run everything against tables that already exist.
    #[test]
    fn the_ledger_key_is_relative_to_the_declared_dir() {
        let files = bundle(&[("db/sql/0001_init.sql", "CREATE TABLE t (id int);")]);
        let got = collect(&files, "db/sql").expect("collect");
        assert_eq!(got[0].filename, "0001_init.sql");
    }

    /// Same bytes → same checksum, and it is the bytes that are hashed, not the
    /// name. Both halves matter: the first is what makes a re-publish a no-op,
    /// the second is what catches an edit.
    #[test]
    fn the_checksum_is_over_the_bytes() {
        let a = collect(&bundle(&[("m/0001.sql", "SELECT 1;")]), "m").unwrap();
        let b = collect(&bundle(&[("m/0009_renamed.sql", "SELECT 1;")]), "m").unwrap();
        let c = collect(&bundle(&[("m/0001.sql", "SELECT 2;")]), "m").unwrap();
        assert_eq!(a[0].checksum, b[0].checksum, "the name must not be hashed");
        assert_ne!(a[0].checksum, c[0].checksum, "the bytes must be hashed");
        assert_eq!(a[0].checksum.len(), 64, "lowercase hex sha256");
    }

    /// Declaring a directory the bundle does not populate is a typo or a build
    /// that forgot to copy it. Silently shipping no schema is the exact failure
    /// this module exists to prevent.
    #[test]
    fn a_declared_dir_with_no_sql_is_an_error_not_an_empty_plan() {
        let files = bundle(&[("index.html", "<html>"), ("migrations/README.md", "hi")]);
        let e = collect(&files, "migrations").expect_err("must refuse");
        assert!(matches!(e, MigrationError::EmptyDir { .. }), "got {e:?}");
        assert!(e.to_string().contains("migrations"), "names the dir");
    }

    /// A `dir` that would sweep the whole bundle, or escape it, is refused
    /// rather than normalised into something plausible.
    #[test]
    fn an_unsafe_dir_is_refused() {
        let files = bundle(&[("m/0001.sql", "SELECT 1;")]);
        for dir in ["", "/", "..", "../m", "/m", "m/../..", "m\\n"] {
            let e = collect(&files, dir).expect_err(&format!("must refuse {dir:?}"));
            assert!(matches!(e, MigrationError::UnsafeDir(_)), "{dir:?}: {e:?}");
        }
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    fn declared(entries: &[(&str, &str)]) -> Vec<DeclaredMigration> {
        entries
            .iter()
            .map(|(name, sql)| DeclaredMigration {
                filename: name.to_string(),
                checksum: sha256_hex(sql.as_bytes()),
                sql: sql.to_string(),
            })
            .collect()
    }

    fn ledger(entries: &[(&str, &str)]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|(name, sql)| (name.to_string(), sha256_hex(sql.as_bytes())))
            .collect()
    }

    #[test]
    fn an_empty_ledger_applies_everything_in_order() {
        let d = declared(&[("0001.sql", "a"), ("0002.sql", "b")]);
        let pending = plan(&d, &HashMap::new()).expect("plan");
        assert_eq!(
            pending.iter().map(|m| &m.filename).collect::<Vec<_>>(),
            vec!["0001.sql", "0002.sql"]
        );
    }

    /// The whole point: a second promote of an unchanged bundle runs nothing.
    /// Not because the SQL is defensive — because the ledger says so.
    #[test]
    fn a_re_promote_of_the_same_bundle_is_a_no_op() {
        let d = declared(&[("0001.sql", "a"), ("0002.sql", "b")]);
        let l = ledger(&[("0001.sql", "a"), ("0002.sql", "b")]);
        assert!(plan(&d, &l).expect("plan").is_empty());
    }

    #[test]
    fn only_the_new_file_runs() {
        let d = declared(&[("0001.sql", "a"), ("0002.sql", "b")]);
        let l = ledger(&[("0001.sql", "a")]);
        let pending = plan(&d, &l).expect("plan");
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].filename, "0002.sql");
    }

    /// THE rule. A file already in the ledger whose bytes changed must fail the
    /// promote and NAME the file — not re-run (divergence) and not skip
    /// (a change the author believes shipped and did not).
    ///
    /// Mutation-tested: flipping `plan`'s checksum comparison to always-equal
    /// turns this into a silent skip and this test fails on `expect_err`.
    #[test]
    fn an_edited_applied_migration_is_a_hard_error_naming_the_file() {
        let d = declared(&[("0001_orders.sql", "CREATE TABLE orders (id int);")]);
        let l = ledger(&[("0001_orders.sql", "CREATE TABLE orders (id bigint);")]);

        let e = plan(&d, &l).expect_err("an edited migration must fail the promote");
        assert!(
            matches!(e, MigrationError::ChecksumMismatch { .. }),
            "got {e:?}"
        );
        let msg = e.to_string();
        assert!(msg.contains("0001_orders.sql"), "must name the file: {msg}");
        // The message has to say what to do instead, or the author's next move
        // is to edit the file again.
        assert!(msg.contains("add a NEW migration file"), "got {msg}");
    }

    /// The same rule reached by renaming rather than editing — the shape that
    /// actually bit us, because a seed's `ON CONFLICT` re-inserts silently
    /// instead of failing with `already exists`.
    #[test]
    fn re_running_applied_sql_under_a_new_name_is_refused() {
        let sql = "INSERT INTO launcher_plan (title) VALUES ('Week 1') ON CONFLICT DO NOTHING;";
        let d = declared(&[("0007_relaunch_plan.sql", sql)]);
        let l = ledger(&[("0002_plan.sql", sql)]);

        let e = plan(&d, &l).expect_err("a renamed applied migration must fail");
        assert!(
            matches!(e, MigrationError::AlreadyAppliedUnderAnotherName { .. }),
            "got {e:?}"
        );
        let msg = e.to_string();
        assert!(msg.contains("0007_relaunch_plan.sql"), "got {msg}");
        assert!(
            msg.contains("0002_plan.sql"),
            "names what it duplicates: {msg}"
        );
    }

    /// A migration deleted from the repo stays applied. The ledger records what
    /// ran; editing the repo cannot un-run it, and re-running the survivors
    /// because a sibling vanished would be gratuitous.
    #[test]
    fn a_file_in_the_ledger_but_not_the_bundle_is_ignored() {
        let d = declared(&[("0002.sql", "b")]);
        let l = ledger(&[("0001_deleted.sql", "a"), ("0002.sql", "b")]);
        assert!(plan(&d, &l).expect("plan").is_empty());
    }

    /// A mismatch must be refused before ANY file runs, including files that
    /// sort before it. Applying 0001 and then refusing 0002 would leave the
    /// promote failed with the tenant already changed.
    #[test]
    fn a_mismatch_refuses_the_whole_plan_not_just_that_file() {
        let d = declared(&[
            ("0001.sql", "a"),
            ("0002.sql", "changed"),
            ("0003.sql", "c"),
        ]);
        let l = ledger(&[("0002.sql", "original")]);
        assert!(plan(&d, &l).is_err(), "0001 must not be planned");
    }

    #[test]
    fn error_classification_drives_the_right_status() {
        assert!(
            MigrationError::ChecksumMismatch {
                filename: "x".into(),
                applied: "a".into(),
                bundled: "b".into(),
            }
            .is_author_fault(),
            "an edited migration is a 4xx — retrying it forever fixes nothing"
        );
        assert!(
            !MigrationError::Busy.is_author_fault() && MigrationError::Busy.is_retryable(),
            "a contended promote is the one case worth retrying"
        );
        assert!(
            !MigrationError::Db("pool exhausted".into()).is_author_fault(),
            "our failure must not be reported as the author's"
        );
    }
}

#[cfg(test)]
mod manifest_tests {
    use super::*;

    /// The common case: an app with no tables declares nothing and pays
    /// nothing. If this ever became an error, every existing app would stop
    /// publishing.
    #[test]
    fn a_manifest_with_no_block_declares_no_migrations() {
        let m = serde_json::json!({ "schemaVersion": 2, "slug": "demo" });
        assert!(migrations_config(Some(&m)).expect("parse").is_none());
        assert!(migrations_config(None).expect("parse").is_none());
    }

    #[test]
    fn a_declared_dir_is_read() {
        let m = serde_json::json!({
            "schemaVersion": 2,
            "slug": "demo",
            "migrations": { "dir": "migrations" }
        });
        let cfg = migrations_config(Some(&m))
            .expect("parse")
            .expect("present");
        assert_eq!(cfg.dir, "migrations");
    }

    /// A present-but-broken block must NOT read as "no migrations" — that
    /// ships code without its tables, which is the whole bug. Unlike the
    /// retention block, whose parse failure safely means "nothing expires".
    #[test]
    fn a_malformed_block_is_an_error_not_an_absence() {
        for bad in [
            serde_json::json!({ "migrations": {} }),
            serde_json::json!({ "migrations": "migrations" }),
            serde_json::json!({ "migrations": { "directory": "migrations" } }),
            serde_json::json!({ "migrations": [] }),
        ] {
            let e = migrations_config(Some(&bad)).expect_err(&format!("must refuse {bad}"));
            assert!(e.contains("dir"), "must say what it wanted: {e}");
        }
    }

    /// An explicit `null` is a generator emitting an unset optional, not a
    /// malformed block.
    #[test]
    fn an_explicit_null_block_declares_nothing() {
        let m = serde_json::json!({ "migrations": serde_json::Value::Null });
        assert!(migrations_config(Some(&m)).expect("parse").is_none());
    }
}
