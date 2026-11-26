//! A copy's pacing, and its key's types and operators.

use oxy_oltp::changes::KeyOrder;

use super::pace;
use super::shape::{TypedColumn, key_types};
use crate::oltp_changes::relay::Limits;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

/// `schema`'s operators, as `KEY_TYPES_SQL` reads a type's.
fn operators(schema: &str) -> KeyOrder {
    KeyOrder {
        lt: format!("{schema}.<"),
        eq: format!("{schema}.="),
        ge: format!("{schema}.>="),
        gt: format!("{schema}.>"),
    }
}

/// Key columns as `KEY_TYPES_SQL` reads them: `(name, type, operators'
/// schema)`.
fn named(columns: &[(&str, &str, &str)]) -> Vec<TypedColumn> {
    columns
        .iter()
        .map(|(name, ty, schema)| TypedColumn {
            name: name.to_string(),
            ty: ty.to_string(),
            order: operators(schema),
        })
        .collect()
}

#[test]
fn the_copy_pauses_at_its_backlog() {
    let l = Limits::default();
    assert_eq!(pace(0, &l), 1000);
    assert_eq!(pace(9_500, &l), 500);
    assert_eq!(pace(10_000, &l), 0);
    assert_eq!(pace(12_000, &l), 0);
}

/// Each key column's type and operators, in the key's order; a key column
/// the table no longer has, or rows out of the key's order, refuse the copy.
#[test]
fn the_key_types_follow_the_key() {
    let key = strings(&["tenant", "id"]);
    let typed = named(&[
        ("tenant", "ext.citext", "ext"),
        ("id", "pg_catalog.int8", "pg_catalog"),
    ]);
    assert_eq!(
        key_types(&key, typed),
        Ok((
            strings(&["ext.citext", "pg_catalog.int8"]),
            vec![operators("ext"), operators("pg_catalog")]
        ))
    );
    let missing = named(&[("id", "pg_catalog.int8", "pg_catalog")]);
    assert!(key_types(&key, missing).is_err());
    let reordered = named(&[
        ("id", "pg_catalog.int8", "pg_catalog"),
        ("tenant", "ext.citext", "ext"),
    ]);
    assert!(key_types(&key, reordered).is_err());
}
