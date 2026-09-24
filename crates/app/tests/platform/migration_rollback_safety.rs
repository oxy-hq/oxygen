//! Fails the build when a migration leaves a schema the *previous* deploy could
//! not live with.
//!
//! ## Why a test
//!
//! Under the deploy train (`internal-docs/deploy-pipeline.md`) a revert is
//! re-pinning the image one step back, and that is worthless if the schema moved
//! forward in a way the older binary cannot read or write. The release doc used
//! to say "any diff in `crates/migration` → stop and ask", which means *the
//! rollback you need most is the one you cannot take*. Nothing about writing a
//! migration stops you from dropping a column, so the objection has to be
//! mechanical.
//!
//! The rule: **a deploy's migrations are additive.** A contraction — dropping a
//! table or column, renaming one, tightening a nullable column to `NOT NULL`,
//! narrowing a type, or *adding* a `UNIQUE` / `CHECK` / key constraint to a table
//! something is already writing — ships in a *later* deploy than the code that
//! stopped needing it. Expand, then contract, one promotion apart.
//!
//! An added constraint belongs on that list even though it removes nothing: it
//! rejects rows the previous deploy is still writing, so the rollback lands on a
//! schema that refuses its traffic. Dropping one is a relaxation and is fine.
//!
//! ## Checked against a migrated database, not source
//!
//! Every migrator chain is stepped one migration at a time against a real
//! Postgres, and `pg_catalog` is diffed after each step. So a raw-SQL migration,
//! a rename and a builder-chain `drop_column` are all accounted for without
//! parsing Rust — the same reasoning `data_placement.rs` applies to table
//! placement. A rename shows up as a drop plus an add, which is correct: the old
//! binary is looking for the old name.
//!
//! ## The cutoff is a date, not a list
//!
//! Every chain names its migrations `m<YYYYMMDD>_<NNNNNN>_<snake>`, including
//! the hand-written ones, so "history" is expressible as a date. Migrations
//! dated before [`RULE_APPLIES_FROM`] are not judged — nobody is going back to
//! re-phase 2026 — and everything from that date on is. A name that does not
//! parse as dated is judged rather than exempted: an accidental exemption is the
//! one failure mode a guard must not have.
//!
//! `airhouse` is not covered. It is a third-party crate on a pinned rev whose
//! migrations are not ours to re-phase, and it exposes a free `up()` rather than
//! a steppable chain. Naming that here makes it a known hole instead of a silent
//! one.
//!
//! ## Proving it detects
//!
//! On a tree whose newest migration predates the cutoff this passes vacuously, and
//! a guard that can only pass is not a guard. Two things close that: the unit
//! tests at the bottom pin each removal shape without a database, and setting
//! [`RULE_APPLIES_FROM`] back to `2020_01_01` makes the live walk report **51**
//! real historical contractions — dropped columns, dropped tables, and one
//! `integer` → `character varying` narrowing in
//! `m20250819_084109_fix_root_replay_ref_type` — each attributed to the migration
//! that made it. Do that if you ever doubt the walk is wired up.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(migration_rollback_safety)'`

use std::collections::{BTreeMap, BTreeSet};

use migration::MigratorTrait;
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};

use crate::common::empty_db;

/// Migrations dated on or after this are judged; earlier ones are history.
///
/// The day this guard landed. Move it only to *open* a window you then close
/// again — never forward to get a failing migration through.
const RULE_APPLIES_FROM: u32 = 2026_09_21;

/// Why a migration is allowed to take something out of the schema.
///
/// `#[allow(dead_code)]` because an empty [`DECLARED`] is the healthy state: no
/// migration since the cutoff has needed to remove anything. The alternative —
/// deleting the variant until it is needed — puts the shape of a correct
/// declaration out of reach of the person reading the failure that asks for one.
#[derive(Clone, Copy)]
#[allow(dead_code)]
enum Why {
    /// Phase 2 of an expand/contract pair. `after` names the migration, PR or
    /// deploy that stopped using what this one removes — that is what a reviewer
    /// checks, and what makes the claim falsifiable.
    Phase2 { after: &'static str },
}

/// Every migration at or after the cutoff that removes something, and why it
/// may. Checked both ways: an undeclared removal fails, and a declaration for a
/// removal that no longer happens fails too, so this cannot drift into
/// describing a schema nobody has.
const DECLARED: &[(&str, &str, Why)] = &[
    // ("central", "m20260930_000001_drop_orders_total",
    //  Why::Phase2 { after: "m20260922_000001_stop_writing_orders_total" }),
];

/// One thing a migration took away.
struct Loss {
    chain: &'static str,
    migration: String,
    what: String,
}

/// The shape of every base table in `public`: its columns, and the rules a write
/// has to satisfy.
///
/// Constraints are in here because *adding* one is a contraction even though it
/// removes nothing — a `UNIQUE`, a `CHECK`, a foreign key or a primary key on an
/// existing table each reject rows the previous deploy is still writing, and the
/// rollback lands on a schema that refuses its traffic. Dropping one is a
/// relaxation and is not flagged.
#[derive(Default)]
struct Snapshot {
    /// Keyed by table + column.
    cols: BTreeMap<(String, String), Column>,
    /// Keyed by table + constraint name, valued by its definition, so a CHECK
    /// rewritten in place is caught as well as one added.
    constraints: BTreeMap<(String, String), String>,
}

#[derive(PartialEq, Eq)]
struct Column {
    nullable: bool,
    /// `format_type` output — `character varying(255)`, `numeric(10,2)`,
    /// `bigint`. One text column is easier to compare than four nullable
    /// `information_schema` fields, and it is what the old binary's driver sees.
    sql_type: String,
}

/// `pg_catalog` rather than `information_schema`: `format_type` gives the whole
/// type in one column, and `attisdropped` keeps Postgres's tombstoned columns
/// out. `relkind = 'r'` is base tables only — a view is not storage.
const SNAPSHOT_SQL: &str = r#"
SELECT c.relname                            AS table_name,
       a.attname                            AS column_name,
       NOT a.attnotnull                     AS nullable,
       format_type(a.atttypid, a.atttypmod) AS sql_type
FROM pg_attribute a
JOIN pg_class c     ON c.oid = a.attrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'public'
  AND c.relkind = 'r'
  AND a.attnum > 0
  AND NOT a.attisdropped
  AND c.relname NOT LIKE 'seaql_migrations%'
ORDER BY 1, 2
"#;

/// Table constraints, plus the unique indexes that are not backing one.
///
/// `CREATE UNIQUE INDEX` and `ADD CONSTRAINT ... UNIQUE` reject the same writes
/// and only one of them lands in `pg_constraint`, so a guard that read only the
/// first would wave the other through. The `conindid` anti-join keeps a
/// constraint's own index from being counted twice under a different name.
const CONSTRAINT_SQL: &str = r#"
SELECT c.relname                        AS table_name,
       con.conname                      AS name,
       pg_get_constraintdef(con.oid)    AS def
FROM pg_constraint con
JOIN pg_class c     ON c.oid = con.conrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'public'
  AND c.relkind = 'r'
  AND c.relname NOT LIKE 'seaql_migrations%'
  -- Postgres 17 gave NOT NULL its own `pg_constraint` row (contype 'n'). The
  -- column snapshot already reads nullability from `attnotnull`, precisely and
  -- without a name attached — and with the name attached, a column rename that
  -- reuses the constraint name reports as "constraint redefined" on top of the
  -- dropped column it already is. One fact, one place.
  AND con.contype <> 'n'
UNION ALL
SELECT c.relname,
       i.relname,
       pg_get_indexdef(i.oid)
FROM pg_index x
JOIN pg_class i     ON i.oid = x.indexrelid
JOIN pg_class c     ON c.oid = x.indrelid
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE n.nspname = 'public'
  AND c.relkind = 'r'
  AND x.indisunique
  AND c.relname NOT LIKE 'seaql_migrations%'
  AND NOT EXISTS (SELECT 1 FROM pg_constraint con WHERE con.conindid = i.oid)
ORDER BY 1, 2
"#;

async fn snapshot(db: &DatabaseConnection) -> Snapshot {
    let cols = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            SNAPSHOT_SQL,
        ))
        .await
        .expect("read the columns from pg_catalog")
        .into_iter()
        .map(|row| {
            let table: String = row.try_get("", "table_name").expect("table_name");
            let column: String = row.try_get("", "column_name").expect("column_name");
            let nullable: bool = row.try_get("", "nullable").expect("nullable");
            let sql_type: String = row.try_get("", "sql_type").expect("sql_type");
            ((table, column), Column { nullable, sql_type })
        })
        .collect();

    let constraints = db
        .query_all_raw(Statement::from_string(
            DatabaseBackend::Postgres,
            CONSTRAINT_SQL,
        ))
        .await
        .expect("read the constraints from pg_catalog")
        .into_iter()
        .map(|row| {
            let table: String = row.try_get("", "table_name").expect("table_name");
            let name: String = row.try_get("", "name").expect("name");
            let def: String = row.try_get("", "def").expect("def");
            ((table, name), def)
        })
        .collect();

    Snapshot { cols, constraints }
}

fn tables(snap: &Snapshot) -> BTreeSet<&str> {
    snap.cols.keys().map(|(t, _)| t.as_str()).collect()
}

/// What `before` had that `after` cannot serve.
fn losses(before: &Snapshot, after: &Snapshot) -> Vec<String> {
    let (gone_tables, still_there) = (
        &tables(before) - &tables(after),
        tables(after).into_iter().collect::<BTreeSet<_>>(),
    );
    let mut out: Vec<String> = gone_tables
        .iter()
        .map(|t| format!("dropped table `{t}`"))
        .collect();

    for ((table, column), was) in &before.cols {
        // A dropped table is already reported; don't also list its columns.
        if !still_there.contains(table.as_str()) {
            continue;
        }
        let Some(now) = after.cols.get(&(table.clone(), column.clone())) else {
            out.push(format!("dropped column `{table}.{column}`"));
            continue;
        };
        if was.nullable && !now.nullable {
            out.push(format!("tightened `{table}.{column}` to NOT NULL"));
        }
        if was.sql_type != now.sql_type && !widens(&was.sql_type, &now.sql_type) {
            out.push(format!(
                "changed `{table}.{column}` from {} to {}",
                was.sql_type, now.sql_type
            ));
        }
    }

    // A rule the previous deploy's writes have never had to satisfy. Only on a
    // table that already existed: a new table arrives with all of its constraints
    // at once, and nothing was writing to it.
    let existed = tables(before);
    for ((table, name), def) in &after.constraints {
        if !existed.contains(table.as_str()) {
            continue;
        }
        match before.constraints.get(&(table.clone(), name.clone())) {
            None => out.push(format!("added `{table}` constraint `{name}` — {def}")),
            Some(was) if was != def => out.push(format!(
                "redefined `{table}` constraint `{name}` — was {was}, now {def}"
            )),
            Some(_) => {}
        }
    }
    out
}

/// Whether `to` can hold everything `from` could. Deliberately a short,
/// explicit list: a type change nobody thought about should fail and be argued,
/// not be waved through by a clever rule.
fn widens(from: &str, to: &str) -> bool {
    const INT_ORDER: [&str; 3] = ["smallint", "integer", "bigint"];
    if let (Some(a), Some(b)) = (
        INT_ORDER.iter().position(|t| *t == from),
        INT_ORDER.iter().position(|t| *t == to),
    ) {
        return b > a;
    }
    if to == "text" && from.starts_with("character varying") {
        return true;
    }
    if let (Some(a), Some(b)) = (
        bounded("character varying", from),
        bounded("character varying", to),
    ) {
        return b > a;
    }
    // `numeric(p,s)` widens when the precision grows and the scale is unchanged;
    // a scale change moves the decimal point and is not a widening.
    if let (Some((p1, s1)), Some((p2, s2))) = (numeric(from), numeric(to)) {
        return s1 == s2 && p2 > p1;
    }
    false
}

/// The `n` in `<name>(n)`, when `s` is exactly that.
fn bounded(name: &str, s: &str) -> Option<u32> {
    s.strip_prefix(name)?
        .trim()
        .strip_prefix('(')?
        .strip_suffix(')')?
        .parse()
        .ok()
}

fn numeric(s: &str) -> Option<(u32, u32)> {
    let inner = s
        .strip_prefix("numeric")?
        .trim()
        .strip_prefix('(')?
        .strip_suffix(')')?;
    let (precision, scale) = inner.split_once(',')?;
    Some((precision.trim().parse().ok()?, scale.trim().parse().ok()?))
}

/// `m20260915_000002_custom_app_migrations_store` → `20260915`. `None` when the
/// name is not dated, which makes the migration subject to the rule.
fn dated(name: &str) -> Option<u32> {
    let digits = name.strip_prefix('m')?.get(..8)?;
    if !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn judged(name: &str) -> bool {
    dated(name).is_none_or(|date| date >= RULE_APPLIES_FROM)
}

/// Steps one chain a migration at a time, collecting what each step removed.
///
/// `baseline` comes in holding whatever earlier chains created and goes out
/// holding this chain's result, so the chains can share one database in the
/// order the server migrates them.
async fn walk<M: MigratorTrait>(
    db: &DatabaseConnection,
    chain: &'static str,
    baseline: &mut Snapshot,
    found: &mut Vec<Loss>,
    seen: &mut BTreeSet<(&'static str, String)>,
) {
    for step in M::migrations() {
        let name = step.name().to_string();
        M::up(db, Some(1))
            .await
            .unwrap_or_else(|e| panic!("{chain} / {name} failed to apply: {e}"));
        let after = snapshot(db).await;
        seen.insert((chain, name.clone()));

        if judged(&name) {
            for what in losses(baseline, &after) {
                found.push(Loss {
                    chain,
                    migration: name.clone(),
                    what,
                });
            }
        }
        *baseline = after;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn migrations_are_rollback_safe() {
    let (db, _url) = empty_db().await;

    let mut baseline = Snapshot::default();
    let mut found: Vec<Loss> = Vec::new();
    let mut seen: BTreeSet<(&'static str, String)> = BTreeSet::new();

    // The order `run_all_migrators` uses (`cli/commands/serve.rs`): the central
    // chain first, then the side migrators that reference its tables.
    walk::<migration::Migrator>(&db, "central", &mut baseline, &mut found, &mut seen).await;
    walk::<agentic_runtime::migration::RuntimeMigrator>(
        &db,
        "runtime",
        &mut baseline,
        &mut found,
        &mut seen,
    )
    .await;
    walk::<agentic_pipeline::AnalyticsMigrator>(
        &db,
        "analytics",
        &mut baseline,
        &mut found,
        &mut seen,
    )
    .await;
    walk::<agentic_pipeline::AutomationMigrator>(
        &db,
        "automation",
        &mut baseline,
        &mut found,
        &mut seen,
    )
    .await;
    walk::<agentic_pipeline::AirwayMigrator>(&db, "airway", &mut baseline, &mut found, &mut seen)
        .await;
    walk::<oxy_oltp::migration::OltpMigrator>(&db, "oltp", &mut baseline, &mut found, &mut seen)
        .await;
    walk::<oxy_cameras::CamerasMigrator>(&db, "cameras", &mut baseline, &mut found, &mut seen)
        .await;

    let declared: BTreeSet<(&str, &str)> = DECLARED.iter().map(|(c, m, _)| (*c, *m)).collect();
    let undeclared: Vec<&Loss> = found
        .iter()
        .filter(|l| !declared.contains(&(l.chain, l.migration.as_str())))
        .collect();

    assert!(
        undeclared.is_empty(),
        "{} schema removal(s) are not declared.\n\n{}\n\n\
         A deploy has to be revertible one step back: the schema it leaves must still be \
         readable and writable by the deploy before it (internal-docs/deploy-pipeline.md). \
         Adding is always safe; removing is not.\n\n\
         Two ways forward, in order of preference:\n\n\
         1. Don't remove it yet. Ship the code that stops using it, let that reach prod, and \
         remove the column in a later deploy. That is the expand/contract pair, and it is the \
         answer nearly every time.\n\n\
         2. If an earlier deploy already stopped using it, declare this as phase 2 in \
         crates/app/tests/platform/migration_rollback_safety.rs:\n\n\
         \x20    (\"{}\", \"{}\", Phase2 {{ after: \"<what stopped using it>\" }}),\n",
        undeclared.len(),
        undeclared
            .iter()
            .map(|l| format!("  {} / {}\n    - {}", l.chain, l.migration, l.what))
            .collect::<Vec<_>>()
            .join("\n"),
        undeclared[0].chain,
        undeclared[0].migration,
    );

    // Both ways. A declaration that no longer describes a removal is a claim
    // about a schema nobody has — and the next real removal in that migration
    // would inherit its exemption.
    let removes: BTreeSet<(&str, &str)> = found
        .iter()
        .map(|l| (l.chain, l.migration.as_str()))
        .collect();
    let stale: Vec<String> = DECLARED
        .iter()
        .filter(|(c, m, _)| !removes.contains(&(*c, *m)))
        .map(|(c, m, _)| {
            if seen.contains(&(*c, (*m).to_string())) {
                format!("  {c} / {m} — the migration applies but removes nothing now")
            } else {
                format!("  {c} / {m} — no such migration in that chain")
            }
        })
        .collect();
    assert!(
        stale.is_empty(),
        "{} declaration(s) no longer describe a removal; delete them:\n{}",
        stale.len(),
        stale.join("\n"),
    );
}

// ── The detector, proven without a database ─────────────────────────────────
//
// The walk above passes vacuously on a tree whose newest migration predates the
// cutoff, and a guard that can only pass is not a guard. These pin the diff
// itself: each case is one removal shape, asserted individually, so a rule that
// stops working fails here rather than going quiet in CI.

#[cfg(test)]
fn snap(cols: &[(&str, &str, bool, &str)]) -> Snapshot {
    Snapshot {
        cols: cols
            .iter()
            .map(|(t, c, nullable, ty)| {
                (
                    ((*t).to_string(), (*c).to_string()),
                    Column {
                        nullable: *nullable,
                        sql_type: (*ty).to_string(),
                    },
                )
            })
            .collect(),
        constraints: BTreeMap::new(),
    }
}

/// [`snap`] plus the rules a write has to satisfy.
#[cfg(test)]
fn snap_with(cols: &[(&str, &str, bool, &str)], constraints: &[(&str, &str, &str)]) -> Snapshot {
    let mut s = snap(cols);
    s.constraints = constraints
        .iter()
        .map(|(t, n, def)| (((*t).to_string(), (*n).to_string()), (*def).to_string()))
        .collect();
    s
}

#[test]
fn additions_are_not_losses() {
    let before = snap(&[("orders", "id", false, "uuid")]);
    let after = snap(&[
        ("orders", "id", false, "uuid"),
        ("orders", "total_cents", true, "bigint"),
        ("refunds", "id", false, "uuid"),
    ]);
    assert!(losses(&before, &after).is_empty());
}

#[test]
fn a_dropped_table_is_reported_once_not_per_column() {
    let before = snap(&[
        ("orders", "id", false, "uuid"),
        ("orders", "total", true, "bigint"),
    ]);
    let after = snap(&[]);
    assert_eq!(losses(&before, &after), vec!["dropped table `orders`"]);
}

#[test]
fn a_dropped_column_is_reported() {
    let before = snap(&[
        ("orders", "id", false, "uuid"),
        ("orders", "total", true, "bigint"),
    ]);
    let after = snap(&[("orders", "id", false, "uuid")]);
    assert_eq!(
        losses(&before, &after),
        vec!["dropped column `orders.total`"]
    );
}

#[test]
fn a_rename_reads_as_a_drop() {
    // The old binary looks for the old name, so a rename is a removal. It also
    // adds a column, which is why only the drop shows up.
    let before = snap(&[("orders", "total", true, "bigint")]);
    let after = snap(&[("orders", "total_cents", true, "bigint")]);
    assert_eq!(
        losses(&before, &after),
        vec!["dropped column `orders.total`"]
    );
}

#[test]
fn tightening_to_not_null_is_a_loss() {
    let before = snap(&[("orders", "note", true, "text")]);
    let after = snap(&[("orders", "note", false, "text")]);
    assert_eq!(
        losses(&before, &after),
        vec!["tightened `orders.note` to NOT NULL"]
    );
}

#[test]
fn relaxing_to_nullable_is_not_a_loss() {
    let before = snap(&[("orders", "note", false, "text")]);
    let after = snap(&[("orders", "note", true, "text")]);
    assert!(losses(&before, &after).is_empty());
}

#[test]
fn narrowing_a_type_is_a_loss_and_widening_is_not() {
    let narrow = losses(
        &snap(&[("orders", "qty", true, "bigint")]),
        &snap(&[("orders", "qty", true, "integer")]),
    );
    assert_eq!(
        narrow,
        vec!["changed `orders.qty` from bigint to integer"],
        "int8 -> int4 loses values"
    );
    assert!(
        losses(
            &snap(&[("orders", "qty", true, "integer")]),
            &snap(&[("orders", "qty", true, "bigint")]),
        )
        .is_empty(),
        "int4 -> int8 holds everything int4 could"
    );
}

#[test]
fn widening_rules_are_exactly_the_ones_declared() {
    assert!(widens("integer", "bigint"));
    assert!(widens("smallint", "integer"));
    assert!(widens("character varying(64)", "text"));
    assert!(widens("character varying(64)", "character varying(128)"));
    assert!(widens("numeric(10,2)", "numeric(12,2)"));

    assert!(!widens("bigint", "integer"));
    assert!(!widens("text", "character varying(64)"));
    assert!(!widens("character varying(128)", "character varying(64)"));
    assert!(
        !widens("numeric(10,2)", "numeric(12,4)"),
        "a scale change moves the decimal point; it is not a widening"
    );
    assert!(
        !widens("timestamp without time zone", "timestamp with time zone"),
        "not on the list, so it has to be argued rather than waved through"
    );
}

#[test]
fn adding_a_constraint_to_an_existing_table_is_a_loss() {
    let cols = &[("orders", "email", true, "text")][..];
    let before = snap(cols);
    let after = snap_with(cols, &[("orders", "orders_email_key", "UNIQUE (email)")]);
    assert_eq!(
        losses(&before, &after),
        vec!["added `orders` constraint `orders_email_key` — UNIQUE (email)"],
        "the previous deploy is still writing duplicates; a rollback lands on a \
         schema that refuses them"
    );
}

#[test]
fn a_new_tables_own_constraints_are_not_losses() {
    let before = snap(&[("orders", "id", false, "uuid")]);
    let after = snap_with(
        &[
            ("orders", "id", false, "uuid"),
            ("refunds", "id", false, "uuid"),
        ],
        &[("refunds", "refunds_pkey", "PRIMARY KEY (id)")],
    );
    assert!(
        losses(&before, &after).is_empty(),
        "nothing was writing to a table that did not exist"
    );
}

#[test]
fn dropping_a_constraint_is_a_relaxation() {
    let cols = &[("orders", "email", true, "text")][..];
    let before = snap_with(cols, &[("orders", "orders_email_key", "UNIQUE (email)")]);
    assert!(losses(&before, &snap(cols)).is_empty());
}

#[test]
fn a_constraint_redefined_in_place_is_a_loss() {
    let cols = &[("orders", "total", true, "bigint")][..];
    let before = snap_with(
        cols,
        &[("orders", "orders_total_check", "CHECK (total >= 0)")],
    );
    let after = snap_with(
        cols,
        &[("orders", "orders_total_check", "CHECK (total > 0)")],
    );
    assert_eq!(
        losses(&before, &after),
        vec![
            "redefined `orders` constraint `orders_total_check` — was CHECK (total >= 0), now CHECK (total > 0)"
        ],
        "a CHECK tightened in place keeps its name and would otherwise be invisible"
    );
}

#[test]
fn the_cutoff_exempts_history_and_judges_everything_else() {
    assert!(!judged("m20260915_000002_custom_app_migrations_store"));
    assert!(!judged("m20260317_000001_create_agentic_tables"));
    assert!(judged("m20260921_000001_landed_the_day_of_the_rule"));
    assert!(judged("m20270101_000001_next_year"));
    assert!(
        judged("DropSomethingUndated"),
        "an unparseable name must be judged; an accidental exemption is the one \
         failure mode a guard cannot have"
    );
}
