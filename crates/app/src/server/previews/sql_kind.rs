//! Which statements in a SQL string write.
//!
//! A workspace preview runs a branch's procedures against real warehouses and
//! must never write to one. [`classify`] is how it tells a read from a write:
//! it parses with the connector's own dialect and answers per statement. Both
//! preview fences read it — the step-level hold (`WorkspaceContext::review_sql`)
//! and the connector backstop ([`super::hold::HoldingConnector`]) — so the two
//! cannot disagree about what a write is.
//!
//! **Fail closed.** `Read` is exactly `airhouse::sql_rules::is_read_only`'s
//! set (`SELECT`/`WITH … SELECT`, `EXPLAIN`, `DESCRIBE`, `SHOW …`), and only
//! when nothing nested in the statement writes: a data-modifying CTE, the body
//! of an `EXPLAIN ANALYZE` and `SELECT … INTO` are writes. Every other
//! statement is a `Write` — session statements (`SET`, `USE`) and `CALL`
//! included, since what they change is not visible from here. SQL that does not
//! parse, or parses to no statement at all, is `Unclassified`, and every caller
//! holds that exactly like a write: only [`is_all_read`] lets SQL through.
//!
//! **Parsed where it cannot crash.** A statement's tree is as deep as its
//! longest operator chain (`a OR b OR …`) or nested type, and parsing, walking
//! or dropping it recurses that deep, so parsing on a Tokio worker's stack is
//! what aborted the process.
//! [`classify`] parses through `airhouse::sql_parse::with_parsed`, as the
//! Airhouse fences do: SQL nested too deep to check is `Unclassified` (held)
//! without being parsed, and deep-but-checkable SQL is parsed and classified
//! on a stack that holds it.
//!
//! **Known limit.** A read that calls a function with side effects
//! (`SELECT my_proc()`) classifies as a read; the parser cannot see inside the
//! function. That is why the connector backstop is not the only fence a preview
//! relies on for a warehouse that allows such functions.

use std::ops::ControlFlow;

use agentic_connector::SqlDialect;
use airhouse::sql_parse::with_parsed;
use airhouse::sql_rules::is_read_only;
use sqlparser::ast::{
    CopySource, Delete, FromTable, Query, SetExpr, Statement, TableFactor, TableObject, Visit,
    Visitor,
};
use sqlparser::dialect::{
    BigQueryDialect, ClickHouseDialect, Dialect, DuckDbDialect, GenericDialect, PostgreSqlDialect,
    SQLiteDialect, SnowflakeDialect,
};
use sqlparser::parser::ParserError;
use sqlparser::tokenizer::{Token, Tokenizer};

/// What one statement does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatementKind {
    Read,
    /// `verb` is what a held-write report shows (`INSERT`, `ALTER TABLE
    /// DELETE`, `SELECT INTO`, …); `targets` the tables it names, when the
    /// statement names any.
    Write {
        verb: String,
        targets: Vec<String>,
    },
    /// Could not be classified; held like a write. The reason says why.
    Unclassified(String),
}

impl StatementKind {
    pub fn is_read(&self) -> bool {
        matches!(self, Self::Read)
    }
}

/// Classify every statement in `sql`, parsed as `dialect` speaks it.
///
/// Never empty: SQL with no statement in it answers one `Unclassified`, so
/// "nothing to classify" can never read as "nothing but reads".
pub fn classify(dialect: SqlDialect, sql: &str) -> Vec<StatementKind> {
    let parser_dialect = parser_dialect(dialect);
    with_parsed(parser_dialect.as_ref(), sql, |parsed| {
        classify_parsed(parser_dialect.as_ref(), sql, &parsed)
    })
    .unwrap_or_else(|too_deep| vec![StatementKind::Unclassified(too_deep.0)])
}

/// [`classify`] for SQL already parsed — inside `with_parsed`, which is the
/// only place a caller that also walks the statements may parse them.
pub(crate) fn classify_parsed(
    dialect: &dyn Dialect,
    sql: &str,
    parsed: &Result<Vec<Statement>, ParserError>,
) -> Vec<StatementKind> {
    match parsed {
        Ok(statements) if statements.is_empty() => vec![StatementKind::Unclassified(
            "no SQL statement found".to_string(),
        )],
        Ok(statements) => statements.iter().map(classify_statement).collect(),
        Err(e) => vec![unparsed(dialect, sql, &e.to_string())],
    }
}

/// The one question a fence asks: may this SQL run against a warehouse a
/// preview must not write to?
pub fn is_all_read(kinds: &[StatementKind]) -> bool {
    !kinds.is_empty() && kinds.iter().all(StatementKind::is_read)
}

/// The first statement that is not a read — what a hold reports.
pub fn first_non_read(kinds: &[StatementKind]) -> Option<&StatementKind> {
    kinds.iter().find(|k| !k.is_read())
}

/// The sqlparser dialect for a connector's [`SqlDialect`]. ClickHouse is
/// matched through [`SqlDialect::CLICKHOUSE`], never its label. Shared with
/// the staging destination fence, so both parse a statement the same way.
/// `Send + Sync` so a parse can move to the big-stack thread.
pub(crate) fn parser_dialect(dialect: SqlDialect) -> Box<dyn Dialect + Send + Sync> {
    if dialect == SqlDialect::CLICKHOUSE {
        return Box::new(ClickHouseDialect {});
    }
    match dialect {
        SqlDialect::Snowflake => Box::new(SnowflakeDialect {}),
        SqlDialect::BigQuery => Box::new(BigQueryDialect {}),
        SqlDialect::Postgres => Box::new(PostgreSqlDialect {}),
        SqlDialect::DuckDb => Box::new(DuckDbDialect {}),
        SqlDialect::Sqlite => Box::new(SQLiteDialect {}),
        SqlDialect::Other(_) => Box::new(GenericDialect {}),
    }
}

fn classify_statement(statement: &Statement) -> StatementKind {
    let mut finder = WriteFinder::default();
    let _ = statement.visit(&mut finder);
    match finder.write {
        None => StatementKind::Read,
        Some((verb, targets)) => StatementKind::Write { verb, targets },
    }
}

/// Walks a statement and everything nested in it — subqueries, CTE bodies, an
/// `EXPLAIN`'s statement — and stops at the first write.
#[derive(Default)]
struct WriteFinder {
    write: Option<(String, Vec<String>)>,
}

impl Visitor for WriteFinder {
    type Break = ();

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<()> {
        if is_read_only(statement) {
            return ControlFlow::Continue(());
        }
        self.write = Some(describe(statement));
        ControlFlow::Break(())
    }

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<()> {
        match select_into(&query.body) {
            Some(target) => {
                self.write = Some(("SELECT INTO".to_string(), vec![target]));
                ControlFlow::Break(())
            }
            None => ControlFlow::Continue(()),
        }
    }
}

/// `SELECT … INTO t` creates `t`. Nested queries are visited on their own.
fn select_into(body: &SetExpr) -> Option<String> {
    match body {
        SetExpr::Select(select) => select.into.as_ref().map(|into| into.name.to_string()),
        SetExpr::SetOperation { left, right, .. } => {
            select_into(left).or_else(|| select_into(right))
        }
        _ => None,
    }
}

/// The verb and target tables a held-write report shows.
fn describe(statement: &Statement) -> (String, Vec<String>) {
    let (verb, targets): (&str, Vec<String>) = match statement {
        Statement::Insert(insert) => ("INSERT", vec![table_object(&insert.table)]),
        Statement::Update(update) => ("UPDATE", vec![factor_name(&update.table.relation)]),
        Statement::Delete(delete) => ("DELETE", delete_targets(delete)),
        Statement::Merge(merge) => ("MERGE", vec![factor_name(&merge.table)]),
        Statement::CreateTable(create) => ("CREATE TABLE", vec![create.name.to_string()]),
        Statement::CreateView(create) => ("CREATE VIEW", vec![create.name.to_string()]),
        Statement::AlterTable(alter) => ("ALTER TABLE", vec![alter.name.to_string()]),
        Statement::Truncate(truncate) => (
            "TRUNCATE",
            truncate
                .table_names
                .iter()
                .map(|t| t.name.to_string())
                .collect(),
        ),
        Statement::OptimizeTable { name, .. } => ("OPTIMIZE", vec![name.to_string()]),
        Statement::RenameTable(renames) => (
            "RENAME TABLE",
            renames
                .iter()
                .flat_map(|r| [r.old_name.to_string(), r.new_name.to_string()])
                .collect(),
        ),
        Statement::Copy {
            source: CopySource::Table { table_name, .. },
            ..
        } => ("COPY", vec![table_name.to_string()]),
        Statement::CreateSchema { schema_name, .. } => {
            ("CREATE SCHEMA", vec![schema_name.to_string()])
        }
        Statement::Drop {
            object_type, names, ..
        } => {
            return (
                format!("DROP {object_type}"),
                names.iter().map(ToString::to_string).collect(),
            );
        }
        other => return (leading_keyword(&other.to_string()), Vec::new()),
    };
    (verb.to_string(), targets)
}

fn table_object(table: &TableObject) -> String {
    match table {
        TableObject::TableName(name) => name.to_string(),
        other => other.to_string(),
    }
}

fn factor_name(factor: &TableFactor) -> String {
    match factor {
        TableFactor::Table { name, .. } => name.to_string(),
        other => other.to_string(),
    }
}

fn delete_targets(delete: &Delete) -> Vec<String> {
    if !delete.tables.is_empty() {
        return delete.tables.iter().map(ToString::to_string).collect();
    }
    match &delete.from {
        FromTable::WithFromKeyword(tables) | FromTable::WithoutKeyword(tables) => {
            tables.iter().map(|t| factor_name(&t.relation)).collect()
        }
    }
}

fn leading_keyword(rendered: &str) -> String {
    rendered
        .split_whitespace()
        .next()
        .unwrap_or("UNKNOWN")
        .to_ascii_uppercase()
}

/// SQL the parser refused. Held either way; this only decides what the hold
/// says. ClickHouse mutations (`ALTER TABLE t DELETE|UPDATE …`) are the one
/// write shape the parser is known not to read, so they are named from the
/// tokens rather than reported as unclassified.
fn unparsed(dialect: &dyn Dialect, sql: &str, error: &str) -> StatementKind {
    let unclassified = || StatementKind::Unclassified(format!("does not parse: {error}"));
    let Ok(tokens) = Tokenizer::new(dialect, sql).tokenize() else {
        return unclassified();
    };
    let tokens: Vec<&Token> = tokens
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    let keyword = |i: usize| match tokens.get(i) {
        Some(Token::Word(w)) => w.value.to_ascii_uppercase(),
        _ => String::new(),
    };
    if keyword(0) != "ALTER" || keyword(1) != "TABLE" {
        return unclassified();
    }
    let mut name = String::new();
    let mut i = 2;
    while let Some(token) = tokens.get(i) {
        match token {
            Token::Word(w) if name.is_empty() || name.ends_with('.') => {
                name.push_str(&w.to_string())
            }
            Token::Period => name.push('.'),
            _ => break,
        }
        i += 1;
    }
    // `ALTER TABLE t [ON CLUSTER c] DELETE|UPDATE …`: the verb follows the
    // name (past an optional ON CLUSTER), or this is not a mutation — a
    // `DELETE` later in the statement (a column, a string) says nothing.
    if keyword(i) == "ON" && keyword(i + 1) == "CLUSTER" {
        i += 3;
    }
    match keyword(i).as_str() {
        verb @ ("DELETE" | "UPDATE") if !name.is_empty() => StatementKind::Write {
            verb: format!("ALTER TABLE {verb}"),
            targets: vec![name],
        },
        _ => unclassified(),
    }
}

#[cfg(test)]
#[path = "sql_kind_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sql_kind_corpus.rs"]
mod corpus;
