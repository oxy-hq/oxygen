//! The host's refusal of a write to the table that woke the writer (app
//! events design §7.3, "Loops").
//!
//! The delivery depth limit cannot see through a table write: a function
//! started by a change to T that writes T starts itself again, one snapshot
//! later, forever. The host holds that write instead, with an error that names
//! the loop. Indirect loops (A writes T1, B watches T1 and writes T2, A
//! watches T2) are the circuit breaker's ([`super::breaker`]).
//!
//! A write is found by parsing the statement ([`written_tables`]); one that
//! does not parse as a plain write is not guessed at, and left to Airhouse's
//! own rules and the breaker. Outside production a write lands in the app's
//! `app_<writer>__<env>` copy, which is the watched table's own copy there, so
//! the comparison is on the schema as the app named it.

use airhouse::app_schema::is_environment_schema;
use airhouse::sql_parse::with_parsed;
use oxy_custom_apps::events::table::TableRef;
use sqlparser::ast::ObjectNamePart;
use sqlparser::dialect::DuckDbDialect;

use crate::custom_apps_functions::env_policy::destination_sql::write_targets;

/// A table a statement writes, as the statement names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    /// The schema part, when the name has one.
    pub schema: Option<String>,
    pub table: String,
}

/// The tables `sql` writes. Empty for SQL that is not one plain write.
pub fn written_tables(sql: &str) -> Vec<Written> {
    let parsed = with_parsed(&DuckDbDialect {}, sql, |parsed| {
        let Ok(statements) = parsed else {
            return Vec::new();
        };
        let mut written = Vec::new();
        for statement in &statements {
            let Ok(targets) = write_targets(statement) else {
                continue;
            };
            for name in targets {
                let parts: Vec<String> = name
                    .0
                    .iter()
                    .filter_map(|p| match p {
                        ObjectNamePart::Identifier(i) => Some(i.value.clone()),
                        _ => None,
                    })
                    .collect();
                if let Some((table, rest)) = parts.split_last() {
                    written.push(Written {
                        schema: rest.last().cloned(),
                        table: table.clone(),
                    });
                }
            }
        }
        written
    });
    parsed.unwrap_or_default()
}

/// The app schema an environment's copy is a sibling of: `app_x__staging` is
/// `app_x`'s; any other name is its own.
fn base_schema(schema: &str) -> &str {
    if is_environment_schema(schema) {
        schema.split_once("__").map_or(schema, |(app, _)| app)
    } else {
        schema
    }
}

/// The refusal for a run started by a change to `watched` that writes one of
/// `written`, or `None`. `own_schema` is what an unqualified name means (the
/// app's `app_<writer>`); `database` is the database the write names (`None`
/// for `ctx.airhouse`, which has none, and matches any watch's).
pub fn refuse(
    watched: &TableRef,
    database: Option<&str>,
    own_schema: Option<&str>,
    written: &[Written],
) -> Option<String> {
    if let (Some(watched_db), Some(db)) = (&watched.database, database)
        && !watched_db.eq_ignore_ascii_case(db)
    {
        return None;
    }
    written.iter().find_map(|w| {
        let schema = w.schema.as_deref().or(own_schema)?;
        let same = base_schema(schema).eq_ignore_ascii_case(&watched.schema)
            && w.table.eq_ignore_ascii_case(&watched.table);
        same.then(|| {
            format!(
                "EventLoop: this run was started by a change to `{}` and writes `{}.{}`, which \
                 would start it again. Write another table, or do this work in a function that \
                 does not watch it",
                watched.spec(),
                schema,
                w.table
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watched() -> TableRef {
        TableRef::parse("pokehouse:bookkeeping.lines").unwrap()
    }

    #[test]
    fn a_write_to_the_watched_table_is_a_loop() {
        for sql in [
            "INSERT INTO bookkeeping.lines VALUES (1)",
            "UPDATE \"bookkeeping\".\"lines\" SET a = 1",
            "DELETE FROM bookkeeping.lines WHERE a = 1",
            "INSERT INTO BOOKKEEPING.Lines SELECT * FROM other.t",
            "TRUNCATE bookkeeping.lines",
        ] {
            let refused = refuse(&watched(), Some("pokehouse"), None, &written_tables(sql));
            assert!(
                refused
                    .as_ref()
                    .is_some_and(|m| m.starts_with("EventLoop:")),
                "{sql}"
            );
        }
    }

    #[test]
    fn a_write_elsewhere_is_not() {
        for sql in [
            "INSERT INTO bookkeeping.other VALUES (1)",
            "INSERT INTO other.lines VALUES (1)",
            "SELECT * FROM bookkeeping.lines",
        ] {
            assert_eq!(
                refuse(&watched(), Some("pokehouse"), None, &written_tables(sql)),
                None,
                "{sql}"
            );
        }
        // A read of the table, in a statement that writes another.
        let w =
            written_tables("INSERT INTO bookkeeping.totals SELECT sum(a) FROM bookkeeping.lines");
        assert_eq!(refuse(&watched(), None, None, &w), None);
    }

    #[test]
    fn another_database_is_another_table() {
        let w = written_tables("INSERT INTO bookkeeping.lines VALUES (1)");
        assert_eq!(refuse(&watched(), Some("warehouse"), None, &w), None);
    }

    #[test]
    fn an_unqualified_write_is_the_apps_own_schema() {
        let own = TableRef::parse("app_bookkeeping.lines").unwrap();
        let w = written_tables("INSERT INTO lines VALUES (1)");
        assert!(refuse(&own, None, Some("app_bookkeeping"), &w).is_some());
        assert!(refuse(&own, None, Some("app_other"), &w).is_none());
    }

    #[test]
    fn an_environments_copy_is_the_same_table() {
        let own = TableRef::parse("app_bookkeeping.lines").unwrap();
        let w = written_tables("INSERT INTO app_bookkeeping__staging.lines VALUES (1)");
        assert!(refuse(&own, None, None, &w).is_some());
    }

    #[test]
    fn sql_that_is_not_one_plain_write_is_left_alone() {
        assert!(written_tables("not sql at all").is_empty());
        assert!(written_tables("SELECT 1").is_empty());
    }
}
