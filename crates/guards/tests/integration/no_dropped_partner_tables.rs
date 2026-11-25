//! Guard: no source file may reference a partner table that no longer exists.
//!
//! The 2026-07-14 permission model dropped `partners`, `partner_members` and
//! `partner_member_orgs` (a partner IS an org now; its people are `org_members`).
//! Sea-ORM call sites broke loudly at compile time — but **two raw SQL queries
//! did not**, and shipped:
//!
//!   admin/orgs_admin.rs   `JOIN partners p ON p.id = po.partner_id`
//!   admin/users_admin.rs  `FROM partner_members pm JOIN partners p ...`
//!
//! Both compiled clean and then 500'd at runtime with
//! `relation "partners" does not exist`, taking down the admin Organizations and
//! Users lists. A green `cargo check` said nothing, because a SQL string is just
//! a string.
//!
//! So this test is the type-checker the raw-SQL escape hatch doesn't have. It
//! reads the crate's own sources and fails if a dropped table name appears in
//! one. Comments and doc-comments are stripped first — the migration and the
//! design notes legitimately *talk about* these tables; what must never appear is
//! a live reference.

use std::fs;
use std::path::Path;

/// Tables the permission-model migration dropped. A hit on any of these in real
/// code means a query that will fail against the current schema.
const DROPPED: [&str; 3] = ["partner_members", "partner_member_orgs", "partners"];

/// Files that are allowed to name the dropped tables in code, not just prose.
const ALLOWED: [&str; 0] = [];

#[test]
fn no_source_references_a_dropped_partner_table() {
    // Workspace-wide (it began as oxy-app only): code is being pulled out into sibling crates
    // (`oxy-app-core`, and the per-surface crates to follow), and a query against a
    // dropped table must be caught wherever it lands. `CARGO_MANIFEST_DIR` is
    // `crates/guards`; its parent is the `crates/` root.
    let src = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/guards has a parent")
        .to_path_buf();
    let mut offenders = Vec::new();
    let mut scanned = 0usize;
    // It anchored on reaching past oxy-app's `app/` until decompose DEL deleted
    // that crate (2026-11-25); now on two crates the walk must reach separately.
    let mut reached_server = false;
    let mut reached_app_core = false;

    visit(&src, &mut |path, text| {
        let rel = path
            .strip_prefix(&src)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/");
        scanned += 1;
        if rel.starts_with("server/src/") {
            reached_server = true;
        }
        if rel.starts_with("app-core/src/") {
            reached_app_core = true;
        }
        if ALLOWED.contains(&rel.as_str()) {
            return;
        }
        for (i, line) in strip_comments(text).lines().enumerate() {
            for table in DROPPED {
                if mentions_table(line, table) {
                    offenders.push(format!("{rel}:{} — {}", i + 1, line.trim()));
                }
            }
        }
    });

    // A scan that silently walks nothing (or only one crate, after the walk root was
    // widened) reports a false green — worse than no test at all. 500 is a floor,
    // not a target: it only has to stay below the real count (~1450 today), so a
    // future split that moves crates out of this workspace lowers it rather than
    // chasing it upward.
    assert!(
        scanned > 500,
        "expected to scan every crate's sources, found only {scanned} files — the walk \
         is broken, and a boundary test that silently scans nothing is worse than no test"
    );
    assert!(
        reached_server && reached_app_core,
        "walk did not cover both crates/server/src and crates/app-core/src — a sibling \
         crate would escape this test"
    );

    assert!(
        offenders.is_empty(),
        "these reference a partner table dropped by the permission model \
         (a partner IS an org; its people are `org_members`). Raw SQL against them \
         compiles fine and then 500s at runtime:\n  {}",
        offenders.join("\n  ")
    );
}

/// `partners` is a substring of `partner_orgs`, `partner_role_bindings`, … and of
/// plenty of live identifiers (`lookup_partners_for_orgs`, `partners.get(..)`), so
/// match on a word boundary AND require it to look like a table reference — i.e.
/// preceded by SQL that would name one.
fn mentions_table(line: &str, table: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    for kw in ["from ", "join ", "into ", "update ", "table "] {
        let mut hay = lower.as_str();
        while let Some(i) = hay.find(kw) {
            let rest = hay[i + kw.len()..].trim_start();
            let word: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if word == table {
                return true;
            }
            hay = &hay[i + kw.len()..];
        }
    }
    // A Sea-ORM path (`entity::partners::`, `prelude::Partners`) would already
    // fail to compile, so SQL is the only real risk — but catch the obvious
    // module path too, in case someone re-adds the entity.
    lower.contains(&format!("entity::{table}::"))
}

/// Drop `//` comments and `/* */` blocks so prose about the old model — which the
/// migration and several doc-comments legitimately contain — doesn't trip the test.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_block = false;
    for line in text.lines() {
        let mut s = line;
        if in_block {
            match s.find("*/") {
                Some(i) => {
                    in_block = false;
                    s = &s[i + 2..];
                }
                None => {
                    out.push('\n');
                    continue;
                }
            }
        }
        let code = match s.find("/*") {
            Some(i) => {
                in_block = !s[i..].contains("*/");
                &s[..i]
            }
            None => s,
        };
        let code = match code.find("//") {
            Some(i) => &code[..i],
            None => code,
        };
        out.push_str(code);
        out.push('\n');
    }
    out
}

/// A guard that only ever passes is indistinguishable from no guard. Pin it to the
/// two lines that actually shipped, so we know it would have caught them — and to
/// the lines that replaced them, so it doesn't cry wolf on the live schema.
#[test]
fn the_guard_catches_the_queries_that_shipped() {
    // Verbatim from the commit that broke the admin Organizations and Users lists.
    let broke_orgs = "FROM partner_orgs po JOIN partners p ON p.id = po.partner_id \\";
    let broke_users = "FROM partner_members pm JOIN partners p ON p.id = pm.partner_id \\";
    assert!(mentions_table(broke_orgs, "partners"));
    assert!(mentions_table(broke_users, "partner_members"));

    // The replacements must NOT trip it: `partners` is a substring of the live
    // table names, so a naive `contains` would flag every one of these.
    for ok in [
        "FROM partner_orgs po JOIN organizations o ON o.id = po.partner_org_id \\",
        "JOIN partner_role_bindings prb ON prb.org_member_id = om.id \\",
        "JOIN partner_grants pg ON pg.org_id = om.org_id \\",
        "let partners = lookup_partners_for_orgs(&db, &org_ids).await?;",
        "partner: partners.get(&org.id).cloned(),",
    ] {
        for table in DROPPED {
            assert!(!mentions_table(ok, table), "false positive on: {ok}");
        }
    }
}

/// Prose about the old model is allowed — only live references are not.
#[test]
fn comments_about_the_old_tables_are_not_references() {
    let src = "// FROM partners p JOIN partner_members pm\nlet x = 1;\n";
    assert!(!strip_comments(src).contains("partners"));
}

fn visit(dir: &Path, f: &mut impl FnMut(&Path, &str)) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n == "target") {
                continue;
            }
            visit(&path, f);
        } else if path.extension().is_some_and(|e| e == "rs")
            // Production sources only. Now that the walk starts at the `crates/` root it
            // would otherwise sweep in every crate's `tests/` tree — including this file,
            // whose `DROPPED` const names all three tables. Mirrors `authz_boundaries`.
            && path.components().any(|c| c.as_os_str() == "src")
            && let Ok(text) = fs::read_to_string(&path)
        {
            f(&path, &text);
        }
    }
}
