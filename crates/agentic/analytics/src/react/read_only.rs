//! The read-only gate in front of customer warehouses.
//!
//! Customer warehouses are read-only to the agent, so `query` runs a statement
//! only when [`check_read_only`] accepts it. The gate fails closed: SQL the
//! dialect's parser cannot read is refused, never guessed at. Refused:
//!
//! - an escape-string prefix starting a token (`E'`, `e'`, `U&'`, `u&'`), in
//!   every dialect. sqlparser's DuckDB and SQLite dialects read `E'\''` as an
//!   identifier and a plain string, while DuckDB reads one escaped quote, so
//!   the rest of the line (`); delete from t; …`) is SQL to the engine and
//!   string to the gate. Postgres E-strings are refused too;
//! - `/*!` (MySQL executable comments) and `/*+` (optimizer hints), in every
//!   dialect: the engine reads their bodies as SQL;
//! - a `/*` inside a block comment. DuckDB nests block comments and sqlparser
//!   does not, so the two would disagree on where the comment ends;
//! - anything but exactly one statement, counted twice: by
//!   [`split_sql_statements`] (what the connectors execute with) and by the
//!   dialect's parser. They must agree. sqlparser reads `#` as a line comment
//!   for MySQL, BigQuery and Snowflake and the splitter does not, so
//!   `select 1 # ; delete from t` is one statement to one and two to the other;
//! - SQL the dialect's parser cannot parse;
//! - any top-level statement that is not a query (DML, DDL, `GRANT`, `COPY`,
//!   `CALL`, `EXPLAIN`, `SET`, …);
//! - a query whose body, anywhere in the tree, is not a plain `SELECT`, a set
//!   operation, a parenthesised query or `VALUES`. That covers a CTE body that
//!   modifies data (Postgres's `WITH d AS (DELETE … RETURNING *) SELECT …`),
//!   at any CTE depth or inside a subquery, and the `TABLE t` shorthand;
//! - any statement nested inside the query (the visitor reaches them inside
//!   `INSERT`/`UPDATE`/`DELETE`/`MERGE` bodies);
//! - `SELECT … INTO` (creates a table in Postgres);
//! - `SELECT … FOR UPDATE` / `FOR SHARE` (takes row locks, needs write rights).
//!
//! Residual risk: the gate trusts each sqlparser dialect, and the connectors'
//! splitter, to tokenize strings, comments and quoting the way the engine does.
//! Where they diverge, text the gate reads as a literal or comment can be SQL
//! to the engine. The checks above close the divergences known today; they
//! cannot prove there are no others. The real backstop is a database-level
//! read-only transaction or role, a follow-up under ruling R8, as is a function
//! with side effects called from a `SELECT` (`select pg_terminate_backend(…)`,
//! `nextval`, a UDF). The gate reads statement shape only.

use std::ops::ControlFlow;

use agentic_connector::{SqlDialect, split_sql_statements};
use sqlparser::ast::{Query, Select, SetExpr, Statement, Visit, Visitor};
use sqlparser::dialect::{
    BigQueryDialect, ClickHouseDialect, Dialect, DuckDbDialect, GenericDialect, MySqlDialect,
    PostgreSqlDialect, SQLiteDialect, SnowflakeDialect,
};
use sqlparser::parser::Parser;

/// The hint for an [`Refusal::EscapeString`]. A plain literal ending in `e`
/// right after a non-letter (`'%e'`, `'e'`) trips the same raw-text scan, so
/// the hint says how to write that `e` without a quote after it.
pub const ESCAPE_STRING_HINT: &str = "use a plain '…' string literal; one ending in e or E \
    right after a non-letter ('e', '%e') is refused too, so build that e with chr(101), \
    e.g. '%' || chr(101)";

/// Why the gate refused a statement.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The dialect's parser could not read it.
    Unparseable(String),
    /// It parsed (or failed a pre-parse check) and is not a single read.
    NotReadOnly(String),
    /// It uses an escape-string literal the dialect parser may misread.
    EscapeString(String),
}

impl Refusal {
    pub fn message(&self) -> &str {
        match self {
            Self::Unparseable(m) | Self::NotReadOnly(m) | Self::EscapeString(m) => m,
        }
    }

    /// What the model should do instead.
    pub fn hint(&self) -> &'static str {
        match self {
            Self::Unparseable(_) => "rewrite as a single standard SELECT",
            Self::NotReadOnly(_) => "only a single SELECT or WITH query runs",
            Self::EscapeString(_) => ESCAPE_STRING_HINT,
        }
    }
}

/// `Ok` when `sql` is a single read statement; `Err` names what was refused.
pub fn check_read_only(sql: &str, dialect: &dyn Dialect) -> Result<(), String> {
    gate(sql, dialect).map_err(|r| r.message().to_owned())
}

/// [`check_read_only`], keeping which kind of refusal it was.
pub fn gate(sql: &str, dialect: &dyn Dialect) -> Result<(), Refusal> {
    check_escape_strings(sql).map_err(Refusal::EscapeString)?;
    check_comments(sql).map_err(Refusal::NotReadOnly)?;
    let split = split_sql_statements(sql).len();
    if split != 1 {
        return Err(Refusal::NotReadOnly(format!(
            "expected exactly one statement, got {split}"
        )));
    }
    let statements = Parser::parse_sql(dialect, sql)
        .map_err(|e| Refusal::Unparseable(format!("could not parse the SQL: {e}")))?;
    check_ast(&statements).map_err(Refusal::NotReadOnly)
}

/// Refuses `E'`/`e'`/`U&'`/`u&'` unless a letter or `_` comes right before
/// (then it ends an identifier such as `name'`, not a prefix).
///
/// Scans the raw text, not tokens: a scanner that knew where literals end
/// would have to agree with every engine on exactly the thing that diverges
/// here. So `x = 'e'` (a quote before `e'`) is refused too, as is a digit
/// before (`1E'`, which an engine may lex as a number then a prefix).
fn check_escape_strings(sql: &str) -> Result<(), String> {
    let chars: Vec<char> = sql.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let prefix_len = match (c, chars.get(i + 1), chars.get(i + 2)) {
            ('E' | 'e', Some('\''), _) => 2,
            ('U' | 'u', Some('&'), Some('\'')) => 3,
            _ => continue,
        };
        let ends_identifier = i > 0 && (chars[i - 1].is_alphabetic() || chars[i - 1] == '_');
        if !ends_identifier {
            let found: String = chars[i..i + prefix_len].iter().collect();
            return Err(format!(
                "escape-string literals (`{found}…'`) are not allowed; a literal ending in \
                 e or E right after a non-letter reads as one"
            ));
        }
    }
    Ok(())
}

/// Refuses executable comments, optimizer hints and nested block comments.
/// Not literal-aware, so a `'/*!'` string is refused too: over-refusing is the
/// safe direction.
fn check_comments(sql: &str) -> Result<(), String> {
    for marker in ["/*!", "/*+"] {
        if sql.contains(marker) {
            return Err(format!("`{marker}` comments are not allowed"));
        }
    }
    let mut in_comment = false;
    let mut rest = sql;
    while let Some(i) = rest.find(['/', '*']) {
        let tail = &rest[i..];
        if tail.starts_with("/*") {
            if in_comment {
                return Err("a `/*` inside a block comment is not allowed".into());
            }
            in_comment = true;
            rest = &tail[2..];
        } else if tail.starts_with("*/") {
            in_comment = false;
            rest = &tail[2..];
        } else {
            rest = &tail[1..];
        }
    }
    Ok(())
}

/// The `sqlparser` dialect for a connector's dialect; Generic for the rest.
pub fn sqlparser_dialect(d: SqlDialect) -> Box<dyn Dialect> {
    match d {
        SqlDialect::Postgres => Box::new(PostgreSqlDialect {}),
        SqlDialect::DuckDb => Box::new(DuckDbDialect {}),
        SqlDialect::Sqlite => Box::new(SQLiteDialect {}),
        SqlDialect::BigQuery => Box::new(BigQueryDialect {}),
        SqlDialect::Snowflake => Box::new(SnowflakeDialect {}),
        d if d == SqlDialect::CLICKHOUSE => Box::new(ClickHouseDialect {}),
        d if d == SqlDialect::MYSQL => Box::new(MySqlDialect {}),
        SqlDialect::Other(_) => Box::new(GenericDialect {}),
    }
}

fn check_ast(statements: &[Statement]) -> Result<(), String> {
    let [statement] = statements else {
        return Err(format!(
            "expected exactly one statement, got {}",
            statements.len()
        ));
    };
    if !matches!(statement, Statement::Query(_)) {
        return Err(format!(
            "{} is not allowed: warehouses are read-only",
            first_word(&statement.to_string())
        ));
    }
    match statement.visit(&mut Gate { statements: 0 }) {
        ControlFlow::Break(reason) => Err(format!("{reason}: warehouses are read-only")),
        ControlFlow::Continue(()) => Ok(()),
    }
}

/// Walks the whole statement; breaks with the reason on the first refusal.
struct Gate {
    statements: usize,
}

impl Visitor for Gate {
    type Break = String;

    fn pre_visit_statement(&mut self, s: &Statement) -> ControlFlow<String> {
        self.statements += 1;
        if self.statements > 1 {
            return ControlFlow::Break(format!(
                "a nested {} statement is not allowed",
                first_word(&s.to_string())
            ));
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_query(&mut self, q: &Query) -> ControlFlow<String> {
        if !q.locks.is_empty() {
            return ControlFlow::Break("SELECT … FOR UPDATE/SHARE takes row locks".into());
        }
        check_body(&q.body)
    }

    fn pre_visit_select(&mut self, s: &Select) -> ControlFlow<String> {
        if s.into.is_some() {
            return ControlFlow::Break("SELECT … INTO creates a table".into());
        }
        ControlFlow::Continue(())
    }
}

/// A query body must be a select, a set operation of them, a nested query or
/// `VALUES`. Nested queries are checked when the visitor reaches them.
fn check_body(body: &SetExpr) -> ControlFlow<String> {
    match body {
        SetExpr::Select(_) | SetExpr::Query(_) | SetExpr::Values(_) => ControlFlow::Continue(()),
        SetExpr::SetOperation { left, right, .. } => {
            check_body(left)?;
            check_body(right)
        }
        SetExpr::Insert(s) | SetExpr::Update(s) | SetExpr::Delete(s) | SetExpr::Merge(s) => {
            ControlFlow::Break(format!(
                "a {} inside a query is not allowed",
                first_word(&s.to_string())
            ))
        }
        SetExpr::Table(_) => ControlFlow::Break("a TABLE query is not allowed; use SELECT".into()),
    }
}

fn first_word(s: &str) -> String {
    s.split_whitespace()
        .next()
        .unwrap_or("statement")
        .to_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every dialect [`sqlparser_dialect`] returns.
    fn dialects() -> Vec<(SqlDialect, Box<dyn Dialect>)> {
        [
            SqlDialect::Postgres,
            SqlDialect::DuckDb,
            SqlDialect::Sqlite,
            SqlDialect::BigQuery,
            SqlDialect::Snowflake,
            SqlDialect::CLICKHOUSE,
            SqlDialect::MYSQL,
            SqlDialect::Other("x"),
        ]
        .into_iter()
        .map(|d| (d, sqlparser_dialect(d)))
        .collect()
    }

    #[test]
    fn read_only_accepts_select_and_with() {
        for (name, d) in dialects() {
            for sql in [
                "select 1",
                "with a as (select 1) select * from a",
                "select 1;",
                "select created_at, updated_at from t union all select 1, 2",
                "select * from t where id in (select id from u)",
                "select 1 /* a comment */ -- and another\n",
            ] {
                assert_eq!(check_read_only(sql, d.as_ref()), Ok(()), "{name}: {sql}");
            }
        }
    }

    #[test]
    fn read_only_rejects_writes() {
        for (name, d) in dialects() {
            for sql in [
                "delete from t",
                "select 1; drop table t",
                "with d as (delete from t returning *) select * from d",
                "with a as (with d as (delete from t returning *) select * from d) select * from a",
                "select * from t where id in (with d as (delete from t returning id) select id from d)",
                "create table x as select 1",
                "select * into x from t",
                "insert into t select 1",
                "update t set a = 1",
                "select * from t for update",
                "",
                // `#` is a comment to sqlparser (MySQL, BigQuery, Snowflake),
                // not to the connectors' splitter.
                "select 1 # ; delete from t",
                // DuckDB nests block comments; sqlparser does not.
                "select 1 /* /* */ , '*/ ); delete from t; select 1 --'",
                "select 1 /*! ; delete from t */",
                "select /*+ no_index */ 1",
                // Unparseable, one statement, carries a write: fails closed.
                "select 1 from t where x = ) delete from t",
                // DuckDB reads `E'\''` as one quote; sqlparser's DuckDB and
                // SQLite dialects read identifier `E` and a plain string.
                r"select E'\'' ); delete from t; select 1 -- '",
                r"select U&'\0027' ); delete from t; select 1 -- '",
                r"select u&'x' ); delete from t; select 1 -- '",
            ] {
                assert!(check_read_only(sql, d.as_ref()).is_err(), "{name}: {sql}");
            }
        }
    }

    #[test]
    fn read_only_refuses_unparseable_sql_with_a_rewrite_hint() {
        let d = PostgreSqlDialect {};
        let r = gate("select created_at from t where x = )", &d).unwrap_err();
        assert!(matches!(r, Refusal::Unparseable(_)), "{r:?}");
        assert_eq!(r.hint(), "rewrite as a single standard SELECT");
        let w = gate("delete from t", &d).unwrap_err();
        assert!(matches!(w, Refusal::NotReadOnly(_)), "{w:?}");
    }

    /// Under MySQL sqlparser reads the whole payload as one statement, so the
    /// splitter's count is what refuses it.
    #[test]
    fn read_only_hash_comment_is_refused_by_the_statement_count() {
        let d = MySqlDialect {};
        let sql = "select 1 # ; delete from t";
        assert_eq!(Parser::parse_sql(&d, sql).map(|s| s.len()).ok(), Some(1));
        let err = check_read_only(sql, &d).unwrap_err();
        assert!(err.contains("exactly one statement, got 2"), "{err}");
    }

    /// The data-modifying CTE parses under Postgres, so the AST walk is what
    /// refuses it.
    #[test]
    fn read_only_ast_walk_refuses_a_data_modifying_cte() {
        let d = PostgreSqlDialect {};
        for sql in [
            "with d as (delete from t returning *) select * from d",
            "with a as (with d as (delete from t returning *) select * from d) select * from a",
        ] {
            let parsed = Parser::parse_sql(&d, sql).expect("postgres parses it");
            let err = check_ast(&parsed).unwrap_err();
            assert!(err.contains("DELETE"), "{sql}: {err}");
        }
    }

    #[test]
    fn read_only_escape_string_prefixes_are_refused_only_at_a_token_start() {
        for sql in [
            "select name', 1",
            "select type_e from t where code = 'abc'",
            "select * from t where note = 'nice'",
        ] {
            assert_eq!(check_escape_strings(sql), Ok(()), "{sql}");
        }
        for sql in [
            r"select E'\''",
            "select e'x'",
            "select U&'x'",
            "select 1 , u&'x'",
        ] {
            assert!(check_escape_strings(sql).is_err(), "{sql}");
        }
        let r = gate(
            r"select E'\'' ); delete from t; select 1 -- '",
            &DuckDbDialect {},
        );
        let r = r.unwrap_err();
        assert_eq!(r.hint(), ESCAPE_STRING_HINT);
        assert!(r.hint().contains("chr(101)"), "{}", r.hint());
        let quote_e = check_escape_strings("select 1 from t where x like '%e'").unwrap_err();
        assert!(quote_e.contains("ending in e or E"), "{quote_e}");
    }

    #[test]
    fn read_only_comment_checks() {
        assert_eq!(check_comments("select 1 /* a */ /* b */"), Ok(()));
        for sql in [
            "select /* /* */ 1",
            "select 1 /*! x */",
            "select /*+ x */ 1",
        ] {
            assert!(check_comments(sql).is_err(), "{sql}");
        }
    }
}
