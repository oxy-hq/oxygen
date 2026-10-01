use sqlparser::dialect::{Dialect, DuckDbDialect, GenericDialect};
use sqlparser::tokenizer::Tokenizer;

use super::{NESTING_UNITS, stack_units};

fn units_in(dialect: &dyn Dialect, sql: &str) -> usize {
    let tokens = Tokenizer::new(dialect, sql)
        .tokenize_with_location()
        .unwrap();
    stack_units(&tokens)
}

fn units(sql: &str) -> usize {
    units_in(&DuckDbDialect {}, sql)
}

#[test]
fn a_chain_counts_one_per_link() {
    let chain = |n: usize| units(&format!("SELECT {}", vec!["1"; n].join(" + ")));
    assert_eq!(chain(1), 0);
    assert_eq!(chain(1_001), 1_000);
    assert_eq!(
        units(&format!("SELECT {}", vec!["a"; 1_001].join(" OR "))),
        1_000
    );
    assert!(units(&format!("SELECT 1{}", "::INT".repeat(1_000))) > 1_000);
    assert!(units(&format!("SELECT x{}", " IS NULL".repeat(1_000))) >= 1_000);
}

/// A set-operation chain runs across commas, so they must not reset it.
#[test]
fn a_union_chain_counts_past_its_commas() {
    let unions = vec!["SELECT 1, 2"; 1_001].join(" UNION ");
    assert_eq!(units(&unions), 1_000);
    let unions_all = vec!["SELECT DISTINCT 1, 2"; 1_001].join(" UNION ALL ");
    assert_eq!(units(&unions_all), 1_000);
    let bracketed = vec!["(SELECT 1, 2)"; 1_001].join(" UNION ");
    assert!(units(&bracketed) > 1_000, "{}", units(&bracketed));
}

#[test]
fn a_bracket_is_a_nesting_level_around_its_contents() {
    assert_eq!(units("SELECT ((((1))))"), 4 * NESTING_UNITS);
    // The inner chain sits under the outer one; siblings do not add up.
    let inner = format!("({})", vec!["1"; 101].join(" + "));
    let outer = format!("SELECT {inner} + {}", vec!["1"; 100].join(" + "));
    assert_eq!(units(&outer), 100 + 100 + NESTING_UNITS);
    assert_eq!(units("SELECT (1), (1), (1)"), NESTING_UNITS);
    // `x[1][2]`: a bracket right after one chains onto it.
    assert_eq!(units("SELECT x[1][1][1]"), 3 * NESTING_UNITS);
    // Unclosed: still counted, never lost.
    assert_eq!(
        units(&format!("SELECT ({}", vec!["1"; 101].join(" + "))),
        100 + NESTING_UNITS
    );
}

/// The type parser recurses with no limit; every level must count a full
/// nesting level, with or without commas.
#[test]
fn a_nested_type_counts_a_nesting_level_per_level() {
    let levels = 100;
    let paren = (0..levels).fold("INT".to_string(), |ty, _| format!("STRUCT(a INT, b {ty})"));
    assert!(units(&format!("SELECT CAST(NULL AS {paren})")) > levels * NESTING_UNITS);
    let suffix = format!("SELECT CAST(NULL AS INT{})", "[]".repeat(levels));
    assert!(units(&suffix) > levels * NESTING_UNITS);
    let angle = (0..levels).fold("INT".to_string(), |ty, _| format!("STRUCT<a INT, b {ty}>"));
    let angle = format!("SELECT CAST(NULL AS {angle})");
    assert!(units_in(&GenericDialect {}, &angle) > levels * NESTING_UNITS);
    // Sibling type columns do not add up; `>>` closes two levels.
    let columns = vec!["c STRUCT<a ARRAY<INT>>"; 50].join(", ");
    let create = format!("CREATE TABLE t ({columns})");
    assert!(units_in(&GenericDialect {}, &create) < 4 * NESTING_UNITS);
    // A comparison is not a type.
    assert!(units("SELECT * FROM t WHERE a < 1 AND b < 2") < 20);
}

/// Big and flat stays small: rows, columns, list items and statements are
/// siblings, and a `CASE`'s branches too.
#[test]
fn a_long_list_or_many_statements_stay_small() {
    let row = "('a', 1, -2.5, NULL, TRUE, '2024-01-01'::TIMESTAMP, 'x', 'y', 'z', 0)";
    let append = format!(
        r#"INSERT INTO "app_x"."t" ("a","b","c","d","e","f","g","h","i","j") VALUES {}"#,
        vec![row; 20_000].join(", ")
    );
    assert!(units(&append) < NESTING_UNITS + 20, "{}", units(&append));
    let ids = (0..50_000).map(|i| format!("'{i}'")).collect::<Vec<_>>();
    let in_list = format!("SELECT * FROM app_x.t WHERE id IN ({})", ids.join(", "));
    assert!(units(&in_list) < NESTING_UNITS + 20, "{}", units(&in_list));
    let migration = vec!["CREATE TABLE app_x.t (id VARCHAR NOT NULL, n INTEGER);"; 5_000];
    let migration = migration.concat();
    assert!(
        units(&migration) < NESTING_UNITS + 20,
        "{}",
        units(&migration)
    );
    let branches = (0..10_000).map(|i| format!("WHEN '{i}' THEN 'c{i}'"));
    let case = format!(
        "SELECT CASE sku {} END FROM t",
        branches.collect::<String>()
    );
    assert!(units(&case) < 20, "{}", units(&case));
}
