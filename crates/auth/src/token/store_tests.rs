//! The runtime mirror and the migration's backfill carry one mapping.

use super::*;

/// The migration, read as source. It is a frozen snapshot that imports no
/// runtime code, and this crate does not depend on `migration`, so the text is
/// the only place the two copies can be compared.
const MIGRATION_SRC: &str = include_str!("../../../migration/src/m20261001_000001_api_tokens.rs");

fn squash(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text after `start`, up to the next `end`.
fn between<'a>(text: &'a str, start: &str, end: &str) -> &'a str {
    let from = text
        .find(start)
        .unwrap_or_else(|| panic!("`{start}` not found"))
        + start.len();
    let len = text[from..]
        .find(end)
        .unwrap_or_else(|| panic!("`{end}` not found after `{start}`"));
    &text[from..from + len]
}

/// The body of the migration's `BACKFILL_SQL` — the constant itself, so a
/// comment that happens to quote the statement cannot stand in for it.
fn migration_backfill() -> &'static str {
    between(MIGRATION_SRC, "pub const BACKFILL_SQL: &str = r#\"", "\"#;")
}

fn migration_columns() -> String {
    squash(between(
        migration_backfill(),
        "INSERT INTO api_tokens (",
        ")",
    ))
}

fn migration_select() -> String {
    squash(between(migration_backfill(), "SELECT", "FROM api_keys k"))
}

/// A SQL list split on its top-level commas (`left(a, 4)` is one item).
fn items(list: &str) -> Vec<String> {
    let mut depth = 0usize;
    let mut out = vec![String::new()];
    for c in list.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                out.push(String::new());
                continue;
            }
            _ => {}
        }
        out.last_mut().expect("starts with one item").push(c);
    }
    out.into_iter().map(|item| squash(&item)).collect()
}

#[test]
fn the_runtime_mirror_fills_the_columns_the_migration_backfill_fills() {
    assert_eq!(
        items(&migration_columns()),
        items(LEGACY_MIRROR_COLUMNS),
        "api_tokens columns: migration backfill (left) vs token::store (right)"
    );
}

#[test]
fn the_runtime_mirror_fills_them_with_the_migrations_expressions() {
    // The one intended difference: the migration stamps its own `source`,
    // where the runtime mirror takes the caller's as `$2`.
    let stamped = format!("'{}'", source::LEGACY_BACKFILL);
    let migration = migration_select();
    assert_eq!(
        migration.matches(&stamped).count(),
        1,
        "the migration stamps its source exactly once"
    );
    assert_eq!(
        items(&migration.replace(&stamped, "$2::text")),
        items(LEGACY_MIRROR_SELECT),
        "expressions over api_keys: migration backfill (left) vs token::store (right)"
    );
}

#[test]
fn every_mirrored_column_has_one_expression() {
    let columns = items(LEGACY_MIRROR_COLUMNS);
    assert_eq!(columns.len(), items(LEGACY_MIRROR_SELECT).len());
    assert_eq!(columns.len(), items(&migration_select()).len());
    assert!(columns.iter().all(|column| !column.is_empty()));
}

#[test]
fn both_runtime_mirrors_are_the_one_statement_under_a_different_filter() {
    for filter in [MIRROR_ONE_KEY, MIRROR_USERS_KEYS] {
        let sql = squash(&mirror_legacy_keys_sql(filter));
        let expected = format!(
            "INSERT INTO api_tokens ( {} ) SELECT {} FROM api_keys k WHERE {filter} \
             ON CONFLICT DO NOTHING",
            squash(LEGACY_MIRROR_COLUMNS),
            squash(LEGACY_MIRROR_SELECT),
        );
        assert_eq!(sql, expected);
    }
}
