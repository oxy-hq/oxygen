//! `scripts/fleet-routes.tsv` against the roles the code declares.
//!
//! `scripts/fleet-assert.sh` reads that table to know which pod has to answer
//! a route, and nothing compared it with the router. So it drifted in the two
//! ways a hand-kept copy does: #3451 moved seven airway routes to `FleetOk`
//! and pinned four by name, and the table went on listing the whole nest as
//! one `ide-only` wildcard; and `/world-model/events` stayed `ide-only` here
//! after its mount became `route_fleet`. A harness reading a stale role
//! asserts the wrong thing about the product, or nothing.
//!
//! What is pinned:
//!
//! - **No row contradicts the code.** A row naming a method + path `oxy-app`
//!   declares must carry the declared role.
//! - **The airway rows are the airway declarations, exactly** — both
//!   directions, against `agentic_http::airway_router_roles()`. The one nest
//!   where a missing row is a failure, because it is the one that was rebuilt
//!   row by row.
//!
//! What is not: completeness of the rest. The table has rows for about two
//! thirds of what `route_declarations()` returns, and a route added elsewhere
//! fails nothing here. Closing that means writing a bucket and a note for
//! every missing route, which is a pass over the harness and not this test.
//!
//! Rows are matched to declarations by exact method + path, not by running
//! `classify` over a made-up concrete path: the table's rows ARE declaration
//! tuples (its header says where they come from), and substituting segments
//! lets a more specific sibling answer for the row being checked.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Where `oxy-app` mounts the airway router, as its declarations spell it.
const AIRWAY_NEST: &str = "/api/{workspace_id}/agentic-airway";

/// Rows naming a method + path that `oxy-app` does not declare, so the role
/// check has nothing to hold them to: the org, team and invitation trees and
/// onboarding (declared by `oxy-api-tenancy` and mounted by `oxy-server`),
/// source uploads (`oxy-api-source-upload`), and a handful of workspace routes
/// that have since been removed or re-nested.
///
/// A backlog with a number on it, not an exemption: a new row that matches no
/// declaration fails here, and so does forgetting to lower this after pruning
/// one — the role check must not be able to skip rows nobody counted.
const ROWS_OXY_APP_DOES_NOT_DECLARE: usize = 32;

/// Every spelling `RouteRole::as_str` produces.
const ROLES: [&str; 3] = ["fleet-ok", "ide-only", "worker-only"];
const BUCKETS: [&str; 5] = ["Now", "Fixture", "Destr", "Ext", "Struct"];
const METHODS: [&str; 6] = ["*", "GET", "POST", "PUT", "PATCH", "DELETE"];

/// One data row of the table, with the line it sits on for the messages.
struct Row {
    line: usize,
    method: String,
    path: String,
    role: String,
    bucket: String,
}

impl Row {
    fn route(&self) -> (String, String) {
        (self.method.clone(), self.path.clone())
    }
}

/// Every data row. Panics on a row that is not five tab-separated columns:
/// the script splits on tabs too, and would read a mangled row as a route with
/// an empty role rather than complain.
fn table() -> Vec<Row> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../scripts/fleet-routes.tsv");
    let src =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    src.lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|(i, l)| {
            let cols: Vec<&str> = l.split('\t').collect();
            assert_eq!(
                cols.len(),
                5,
                "fleet-routes.tsv:{}: expected method, path, role, bucket, notes \
                 separated by tabs, got {} column(s)",
                i + 1,
                cols.len()
            );
            Row {
                line: i + 1,
                method: cols[0].to_string(),
                path: cols[1].to_string(),
                role: cols[2].to_string(),
                bucket: cols[3].to_string(),
            }
        })
        .collect()
}

/// `(method, path) -> role` for every route `oxy-app` declares, both modes.
fn declared() -> BTreeMap<(String, String), &'static str> {
    oxy_app::server::router::route_declarations()
        .into_iter()
        .map(|(method, path, role)| ((method.to_string(), path), role.as_str()))
        .collect()
}

#[test]
fn every_row_is_one_the_script_can_read() {
    let rows = table();
    // The script refuses a table under 200 rows as truncated.
    assert!(rows.len() >= 200, "only {} rows parsed", rows.len());

    let mut seen = BTreeSet::new();
    for row in &rows {
        let at = format!("fleet-routes.tsv:{}", row.line);
        assert!(
            METHODS.contains(&row.method.as_str()),
            "{at}: method {:?}",
            row.method
        );
        assert!(
            row.path.starts_with('/'),
            "{at}: path {:?} is not absolute",
            row.path
        );
        assert!(
            ROLES.contains(&row.role.as_str()),
            "{at}: role {:?} is none of {ROLES:?}",
            row.role
        );
        assert!(
            BUCKETS.contains(&row.bucket.as_str()),
            "{at}: bucket {:?} is none of {BUCKETS:?} — the coverage report \
             would drop the row from every total",
            row.bucket
        );
        assert!(
            seen.insert(row.route()),
            "{at}: {} {} is listed twice",
            row.method,
            row.path
        );
    }
}

#[test]
fn no_row_contradicts_the_role_the_code_declares() {
    let declared = declared();
    assert!(
        declared.len() > 200,
        "route_declarations() returned {} routes — the comparison below would \
         be checking next to nothing",
        declared.len()
    );

    let mut wrong = Vec::new();
    let mut not_declared = Vec::new();
    for row in table() {
        match declared.get(&row.route()) {
            Some(role) if *role == row.role => {}
            Some(role) => wrong.push(format!(
                "fleet-routes.tsv:{}: {} {} is `{}` in the table and `{role}` in the code",
                row.line, row.method, row.path, row.role
            )),
            None => not_declared.push(format!("{} {}", row.method, row.path)),
        }
    }

    assert!(
        wrong.is_empty(),
        "the table states a role the code does not declare. The code is the \
         authority — fix the row (and its note, if the note explains the old \
         role):\n{}",
        wrong.join("\n")
    );
    assert_eq!(
        not_declared.len(),
        ROWS_OXY_APP_DOES_NOT_DECLARE,
        "rows naming a method + path oxy-app does not declare changed from \
         {ROWS_OXY_APP_DOES_NOT_DECLARE}. A new one is a typo, a route that \
         moved to a surface crate, or one that was removed — the role check \
         cannot see any of the three. Fix the row, or change the count with \
         the reason. Currently undeclared:\n{}",
        not_declared.join("\n")
    );
}

#[test]
fn the_airway_rows_are_exactly_the_airway_declarations() {
    let in_code: BTreeSet<(String, String, String)> = agentic_http::airway_router_roles()
        .iter()
        .map(|d| {
            (
                d.method.to_string(),
                format!("{AIRWAY_NEST}{}", d.path),
                d.role.as_str().to_string(),
            )
        })
        .collect();
    assert!(
        in_code.len() > 1,
        "airway_router_roles() declares {} route(s) — nothing to compare",
        in_code.len()
    );

    let nest = format!("{AIRWAY_NEST}/");
    let in_table: BTreeSet<(String, String, String)> = table()
        .into_iter()
        .filter(|row| row.path.starts_with(&nest))
        .map(|row| (row.method, row.path, row.role))
        .collect();

    let missing: Vec<_> = in_code.difference(&in_table).collect();
    let stale: Vec<_> = in_table.difference(&in_code).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "scripts/fleet-routes.tsv and agentic_http::airway_router_roles() \
         disagree about the airway nest.\n\
         declared, with no matching row (add it, with a bucket and a note): {missing:#?}\n\
         rows matching no declaration (the route's method, path or role changed): {stale:#?}"
    );
}
