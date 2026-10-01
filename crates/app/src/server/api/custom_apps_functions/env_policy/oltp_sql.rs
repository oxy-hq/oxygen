//! Which `ctx.oltp` statements a non-production run may send to production's
//! OLTP store: **one read, and nothing else**.
//!
//! A `READ ONLY` transaction alone does not hold a write. A statement can end
//! it (`COMMIT`, after which the next one autocommits on the writer role) or
//! lift it (`SET TRANSACTION READ WRITE` as the first statement, before a
//! snapshot exists) — both reproduced against Postgres in the Phase 3 review.
//! So every statement is classified first, with the previews classifier
//! (`previews::sql_kind::classify`, Postgres dialect), and only a single
//! `Read` is sent. Transaction control, session statements, `DO`, `CALL`,
//! `COPY`, `LOCK`, `LISTEN`/`NOTIFY`, several statements in one string, and
//! SQL that does not parse are all held.
//!
//! A read can still call a function with side effects, which the parser
//! cannot see inside. The ones Postgres ships are refused by name
//! ([`side_effect`]): case-folded, whether quoted or schema-qualified. So are
//! the ones that run SQL handed to them as text (`query_to_xml`, `ts_stat`,
//! `ts_rewrite`), since no name inside a string literal is read. A name
//! spelled with Unicode escapes (`U&"set_confi\0067"`) cannot be read off the
//! tokens at all — the parser even takes it for `U & "…"(…)`, one read — so
//! any `U&` quoted identifier is held outright.
//!
//! A function the app defined itself is what the `READ ONLY` transaction and
//! the session's `default_transaction_read_only` are still there for, and
//! they refuse the writes such a function makes. **They do not refuse
//! everything:** `READ ONLY` allows `pg_notify` and advisory locks, so an
//! app-defined function that calls either passes all three layers — a staging
//! run can take a lock production waits on, or wake production's listeners.
//! Nothing here sees inside an app-defined function; the OLTP staging branch
//! (previews P4b) is what ends it, by giving staging a database of its own.

use agentic_connector::SqlDialect;
use sqlparser::dialect::{Dialect, PostgreSqlDialect};
use sqlparser::tokenizer::{Token, Tokenizer};

use crate::server::previews::sql_kind::{StatementKind, classify};

/// A statement held rather than sent. `verb` and `table` go in the held row
/// (identifiers only); `why` finishes the error the function sees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldStatement {
    pub verb: String,
    pub table: String,
    pub why: String,
}

/// Built-in functions that change state outside the statement that calls
/// them: session settings, other connections, locks, notifications, large
/// objects, sequences.
const SIDE_EFFECT_FUNCTIONS: &[&str] = &[
    "set_config",
    "pg_notify",
    "nextval",
    "setval",
    "pg_terminate_backend",
    "pg_cancel_backend",
];

/// The same, by family.
/// `system$` is Snowflake's family of system functions (`SYSTEM$CANCEL_QUERY`,
/// `SYSTEM$ABORT_SESSION`, …), shared with the mapped-destination fence.
const SIDE_EFFECT_PREFIXES: &[&str] =
    &["dblink", "pg_advisory", "pg_try_advisory", "lo_", "system$"];

/// Built-in functions that run SQL handed to them as a string, which no check
/// here reads into: `query_to_xml('select pg_notify(…)', …)` notifies, inside
/// a `READ ONLY` transaction, as `ts_stat` and `ts_rewrite` do (each verified
/// against Postgres 16).
pub(super) const SQL_TEXT_FUNCTIONS: &[&str] = &["ts_stat", "ts_rewrite"];

/// The same, by family: `query_to_xml`, `query_to_xmlschema`,
/// `query_to_xml_and_xmlschema`.
pub(super) const SQL_TEXT_PREFIXES: &[&str] = &["query_to_xml"];

/// `Ok` when `sql` may be sent: exactly one statement, a read, calling no
/// side-effect function.
pub fn admit_oltp_statement(sql: &str) -> Result<(), HeldStatement> {
    let kinds = classify(SqlDialect::Postgres, sql);
    match kinds.as_slice() {
        [StatementKind::Read] => match side_effect(&PostgreSqlDialect {}, sql) {
            None => Ok(()),
            Some(why) => Err(HeldStatement {
                verb: "SELECT".to_string(),
                table: String::new(),
                why,
            }),
        },
        [StatementKind::Write { verb, targets }] => Err(HeldStatement {
            verb: verb.clone(),
            table: targets.first().cloned().unwrap_or_default(),
            why: format!("is a {verb}"),
        }),
        [StatementKind::Unclassified(reason)] => Err(HeldStatement {
            verb: "UNCLASSIFIED".to_string(),
            table: String::new(),
            why: format!("could not be classified ({reason})"),
        }),
        many => Err(HeldStatement {
            verb: "MULTIPLE".to_string(),
            table: String::new(),
            why: format!("is {} statements in one string", many.len()),
        }),
    }
}

/// Why a read is held anyway — the rest of the held error, after "this one" —
/// or `None` to send it. Held: a call to a listed function (a name followed
/// by `(`), a name spelled with Unicode escapes, or SQL that does not
/// tokenize. The last part of a schema-qualified name is the one compared,
/// case-folded whether quoted or not, so `"SET_CONFIG"` and
/// `pg_catalog.set_config` are both caught. A string literal or a comment is
/// not a name.
///
/// `dialect` is how the SQL is tokenized: Postgres for `ctx.oltp`, the mapped
/// connector's own dialect on a mapped destination's read path. The Unicode
/// escape rule reads a Postgres spelling; on another dialect it can only hold
/// more (a quoted identifier after `u &`), never less.
pub(super) fn side_effect(dialect: &dyn Dialect, sql: &str) -> Option<String> {
    let Ok(tokens) = Tokenizer::new(dialect, sql).tokenize() else {
        return Some("could not be read for the functions it calls".to_string());
    };
    let tokens: Vec<&Token> = tokens
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    if names_a_unicode_escape(&tokens) {
        return Some(
            "spells a name with Unicode escapes (U&\"…\"), so what it names cannot be checked"
                .to_string(),
        );
    }
    tokens.windows(2).find_map(|pair| match pair {
        [Token::Word(word), Token::LParen] => called(&word.value.to_ascii_lowercase()),
        _ => None,
    })
}

/// Why calling `name` (case-folded) holds a read, or `None` when it does not.
fn called(name: &str) -> Option<String> {
    if listed(name, SQL_TEXT_FUNCTIONS, SQL_TEXT_PREFIXES) {
        Some(format!(
            "calls {name}(), which runs SQL given as text that is not checked"
        ))
    } else if listed(name, SIDE_EFFECT_FUNCTIONS, SIDE_EFFECT_PREFIXES) {
        Some(format!(
            "calls {name}(), which changes state outside the statement"
        ))
    } else {
        None
    }
}

/// `U&"…"`: an identifier spelled with Unicode escapes, which Postgres
/// decodes and the tokenizer does not — it reads an unquoted `U`, `&`, and a
/// quoted word. Whitespace between them is ignored, so `u & "col"` is held
/// too: a false hold, never a missed one.
pub(super) fn names_a_unicode_escape(tokens: &[&Token]) -> bool {
    tokens.windows(3).any(|triple| {
        matches!(
            triple,
            [Token::Word(u), Token::Ampersand, Token::Word(quoted)]
                if u.quote_style.is_none()
                    && u.value.eq_ignore_ascii_case("u")
                    && quoted.quote_style.is_some()
        )
    })
}

pub(super) fn listed(name: &str, names: &[&str], prefixes: &[&str]) -> bool {
    names.contains(&name) || prefixes.iter().any(|prefix| name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(sql: &str) -> HeldStatement {
        admit_oltp_statement(sql).expect_err(sql)
    }

    #[test]
    fn a_single_read_is_sent() {
        for sql in [
            "select count(*)::int as n from orders",
            "SELECT * FROM orders WHERE note = 'set_config(' -- nextval(",
            "with x as (select 1 as id) select id from x",
            "SHOW default_transaction_read_only",
            "EXPLAIN SELECT 1",
            "select 1;",
        ] {
            assert_eq!(admit_oltp_statement(sql), Ok(()), "{sql}");
        }
    }

    /// The review's escapes: ending the read-only transaction, or lifting it
    /// before the first snapshot.
    #[test]
    fn transaction_and_session_control_is_held() {
        for sql in [
            "COMMIT",
            "commit; insert into orders values (2)",
            "SET TRANSACTION READ WRITE",
            "SET SESSION CHARACTERISTICS AS TRANSACTION READ WRITE",
            "set default_transaction_read_only = off",
            "BEGIN",
            "START TRANSACTION",
            "END",
            "ROLLBACK",
            "ABORT",
            "SAVEPOINT s",
            "RELEASE SAVEPOINT s",
            "RESET ALL",
            "DISCARD ALL",
            "PREPARE p AS SELECT 1",
            "LISTEN channel",
            "NOTIFY channel",
            "LOCK TABLE orders",
            "DO $$ BEGIN INSERT INTO orders VALUES (9); END $$",
            "CALL write_things()",
            "COPY orders TO STDOUT",
        ] {
            held(sql);
        }
    }

    #[test]
    fn writes_several_statements_and_unparsed_sql_are_held() {
        let insert = held("insert into orders (id) values (2)");
        assert_eq!(
            (insert.verb.as_str(), insert.table.as_str()),
            ("INSERT", "orders")
        );
        let cte =
            held("with x as (insert into orders (id) values (3) returning id) select id from x");
        assert_eq!(cte.verb, "INSERT");
        assert_eq!(held("select 1; select 2").verb, "MULTIPLE");
        assert_eq!(held("selec 1 frm").verb, "UNCLASSIFIED");
        assert_eq!(held("").verb, "UNCLASSIFIED");
    }

    #[test]
    fn a_read_calling_a_side_effect_function_is_held() {
        for sql in [
            "select set_config('default_transaction_read_only', 'off', false)",
            "SELECT pg_catalog.set_config('x', 'y', true)",
            "select \"set_config\"('x', 'y', true)",
            "select * from dblink('host=x', 'select 1') as t(a int)",
            "select dblink_exec('host=x', 'insert into t values (1)')",
            "select pg_advisory_lock(1)",
            "select pg_try_advisory_xact_lock(1)",
            "select pg_notify('c', 'p')",
            "select lo_import('/etc/passwd')",
            "select nextval('orders_id_seq')",
            "select setval('orders_id_seq', 1)",
            "select pg_terminate_backend(42)",
            "select pg_cancel_backend(42)",
        ] {
            let statement = held(sql);
            assert!(statement.why.starts_with("calls "), "{sql}: {statement:?}");
        }
    }

    /// A name is compared case-folded and by its last part, quoted or not.
    #[test]
    fn a_side_effect_name_is_caught_however_it_is_spelled() {
        for sql in [
            "select \"SET_CONFIG\"('x', 'y', true)",
            "SELECT Set_Config('x', 'y', true)",
            "select pg_catalog.set_config('x', 'y', true)",
            "select \"pg_catalog\".\"set_config\"('x', 'y', true)",
            "select public.pg_notify('c', 'p')",
            "select PG_CATALOG.PG_ADVISORY_LOCK(1)",
            "select set_config /* spaced */ ('x', 'y', true)",
        ] {
            let statement = held(sql);
            assert!(statement.why.starts_with("calls "), "{sql}: {statement:?}");
        }
    }

    /// Reproduced in the round-2 review: Postgres decodes `U&"…"` and runs
    /// `set_config`, while the parser reads `U & f(…)` — one read.
    #[test]
    fn a_unicode_escaped_name_is_held_whatever_it_names() {
        for sql in [
            r#"SELECT U&"set_confi\0067"('default_transaction_read_only', 'off', false)"#,
            r#"select u&"pg_notif\0079"('c', 'p')"#,
            r#"select pg_catalog.U&"set_confi\0067"('x', 'y', true)"#,
            r#"select U&"pg_advisory_lock"(1)"#,
            // A false hold, by design: whitespace is not read.
            r#"select u & "b" from t"#,
        ] {
            let statement = held(sql);
            assert!(
                statement.why.contains("Unicode escapes"),
                "{sql}: {statement:?}"
            );
        }
        // With `UESCAPE`, the parser gives up: held as unclassified.
        held(r#"select U&"d!0061t!+000061" UESCAPE '!' from orders"#);
        // A column that merely ANDs with a quoted one is not an escape, and a
        // `U&'…'` string is a literal, not a name.
        assert_eq!(admit_oltp_statement(r#"select a & "b" from t"#), Ok(()));
        assert_eq!(admit_oltp_statement(r#"select U&'d\0061t' as s"#), Ok(()));
    }

    /// A function that runs SQL given as text carries any name past the check
    /// inside its string; each of these ran `set_config` or `pg_notify` in a
    /// `READ ONLY` transaction on Postgres 16.
    #[test]
    fn a_read_calling_a_function_that_runs_sql_text_is_held() {
        for sql in [
            "select query_to_xml('select pg_notify(''c'', ''p'')', true, false, '')",
            "select pg_catalog.Query_To_Xml_And_XmlSchema('select 1', true, false, '')",
            "select * from ts_stat('select to_tsvector(set_config(''a.b'', ''c'', false))')",
            "select ts_rewrite('a'::tsquery, 'select ''a''::tsquery, ''b''::tsquery')",
        ] {
            let statement = held(sql);
            assert!(
                statement.why.contains("runs SQL given as text"),
                "{sql}: {statement:?}"
            );
        }
        // The read-only relatives that take a table, not text, are sent.
        assert_eq!(
            admit_oltp_statement("select table_to_xml('orders', true, false, '')"),
            Ok(())
        );
    }
}
