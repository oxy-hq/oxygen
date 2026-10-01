//! Which statements a staging write may send on a **mapped destination** —
//! the connection `nonProduction.destinations` isolated it to.
//!
//! The mapping changes where the connection goes, not what a statement names:
//! `INSERT INTO PROD_DB.PUBLIC.ORDERS …` sent on the mapped connection lands in
//! production whenever that credential can write production's database. And
//! the ways a statement can write somewhere else are open-ended — `ALTER TABLE
//! … SWAP WITH`, `RENAME TO`, a materialized view `TO` another table, a
//! `Distributed` or `Buffer` engine, `SELECT … INTO`. So this is an
//! **allowlist**: a statement is sent only when it is
//!
//! - one read (reads may name any database — staging reads production by
//!   design), or
//! - one `INSERT`, `UPDATE`, `DELETE`, `MERGE`, `TRUNCATE`, `DROP TABLE` or
//!   `CREATE TABLE` (with no engine clause, or a ClickHouse MergeTree-family,
//!   `Memory` or `Log`-family engine; `AS SELECT` allowed), writing nothing in
//!   its sources,
//!
//! and every table it **writes** is unqualified or qualified by the mapped
//! entry's configured database — never by a production database the mapping
//! names. Everything else is held unsent: `ALTER`, views, other engines,
//! `ATTACH`/`USE`/`SET`/`PRAGMA`/`COPY`, several statements in one string, and
//! SQL that does not parse (`previews::sql_kind::classify` decides read from
//! write, in the connector's dialect).
//!
//! Which part of a name is a database is the dialect's: `db.schema.table` for
//! Postgres and Snowflake; `db.table` for ClickHouse and MySQL; DuckDB's
//! `x.table` only when `x` is the mapped catalog or `main`; BigQuery's
//! `dataset.table` only when the dataset is one the mapped entry configures and
//! no production entry does, and never a `project.dataset.table`. A quoted
//! name is compared exactly; an unquoted one without case.
//!
//! A fence against a statement reaching past the mapping, not proof of
//! isolation: the staging credential must hold no write grant on production
//! (`internal-docs/customer-apps-functions.md`, Staging).

use std::ops::ControlFlow;

use agentic_connector::SqlDialect;
use airhouse::sql_parse::with_parsed;
use sqlparser::ast::{
    CreateTable, CreateTableOptions, FromTable, ObjectName, ObjectNamePart, ObjectType,
    OutputClause, Query, SetExpr, SqlOption, Statement, TableFactor, TableObject, TableWithJoins,
    Visit, Visitor,
};
use sqlparser::dialect::Dialect;
use sqlparser::parser::ParserError;

use super::HeldStatement;
use super::oltp_sql::side_effect;
use crate::server::previews::sql_kind::{StatementKind, classify_parsed, parser_dialect};

/// What a write on a mapped connection may name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DestinationFence {
    /// The mapped connector's dialect — how its SQL parses.
    pub dialect: SqlDialect,
    /// What a write target may be qualified by: the mapped entry's configured
    /// database (BigQuery: its datasets), as configured.
    pub mapped: Vec<String>,
    /// What a write target may never be qualified by: every production
    /// database the mapping names (BigQuery: every production dataset).
    pub production: Vec<String>,
}

/// `Ok` when `sql` may be sent on the mapped connection. Parsed once, the way
/// `previews::sql_kind::classify` parses — SQL nested too deep to check is
/// held unparsed, and the statement is walked and dropped on a stack that
/// holds it.
pub fn admit_mapped_statement(fence: &DestinationFence, sql: &str) -> Result<(), HeldStatement> {
    let dialect = parser_dialect(fence.dialect);
    with_parsed(dialect.as_ref(), sql, |parsed| {
        let kinds = classify_parsed(dialect.as_ref(), sql, &parsed);
        let statement = one_statement(parsed, &kinds)?;
        admit_one(fence, dialect.as_ref(), sql, kinds[0].is_read(), &statement)
    })
    .unwrap_or_else(|too_deep| Err(unclassified(&too_deep.0)))
}

/// [`admit_mapped_statement`] for the one statement `sql` parsed to.
fn admit_one(
    fence: &DestinationFence,
    dialect: &dyn Dialect,
    sql: &str,
    is_read: bool,
    statement: &Statement,
) -> Result<(), HeldStatement> {
    if is_read {
        // A read that calls a function with side effects is not a read.
        return match side_effect(dialect, sql) {
            None => Ok(()),
            Some(why) => Err(held("SELECT", "", why)),
        };
    }
    let verb = verb(statement);
    if outputs_into_a_table(statement) {
        return Err(held(
            &verb,
            "",
            "writes a second table with OUTPUT … INTO".into(),
        ));
    }
    let targets = write_targets(statement).map_err(|why| held(&verb, "", why))?;
    if let ControlFlow::Break(why) = statement.visit(&mut NestedWrites::default()) {
        return Err(held(&verb, "", why));
    }
    for target in targets {
        check_target(fence, target).map_err(|why| held(&verb, &target.to_string(), why))?;
    }
    Ok(())
}

/// The error a statement held on a mapped destination throws, after the held
/// message.
pub fn mapped_statement_suffix(why: &str) -> String {
    format!(
        " On a mapped destination only a read, or one INSERT, UPDATE, DELETE, MERGE, TRUNCATE, \
         DROP TABLE or CREATE TABLE writing only the mapped database, is sent; this one {why}."
    )
}

/// The one statement `parsed` holds — or why it is held.
fn one_statement(
    parsed: Result<Vec<Statement>, ParserError>,
    kinds: &[StatementKind],
) -> Result<Statement, HeldStatement> {
    match (parsed, kinds) {
        (_, [StatementKind::Unclassified(reason)]) => Err(unclassified(reason)),
        (Ok(mut statements), [_]) if statements.len() == 1 => Ok(statements.remove(0)),
        (_, [_]) => Err(held("UNCLASSIFIED", "", "does not parse".to_string())),
        (_, many) => Err(held(
            "MULTIPLE",
            "",
            format!("is {} statements in one string", many.len()),
        )),
    }
}

/// The tables an allowed write writes, or why the statement is not allowed.
fn write_targets(statement: &Statement) -> Result<Vec<&ObjectName>, String> {
    fn one<'a>(name: Option<&'a ObjectName>, why: &str) -> Result<Vec<&'a ObjectName>, String> {
        name.map(|n| vec![n]).ok_or(why.to_string())
    }
    match statement {
        // Snowflake's `INSERT ALL|FIRST … INTO a … INTO b` parses with its
        // targets in the multi-table clauses and `table` left empty.
        Statement::Insert(insert)
            if insert.multi_table_insert_type.is_some()
                || !insert.multi_table_into_clauses.is_empty()
                || !insert.multi_table_when_clauses.is_empty()
                || insert.multi_table_else_clause.is_some() =>
        {
            Err("is a multi-table INSERT, which writes several tables".into())
        }
        Statement::Insert(insert) => match &insert.table {
            TableObject::TableName(name) => Ok(vec![name]),
            _ => Err("inserts into a table function, which can reach another server".into()),
        },
        Statement::Update(update) => one(sole_table(&update.table), "updates through a join"),
        Statement::Delete(delete) if !delete.tables.is_empty() => {
            Ok(delete.tables.iter().collect())
        }
        Statement::Delete(delete) => match &delete.from {
            FromTable::WithFromKeyword(tables) | FromTable::WithoutKeyword(tables) => tables
                .iter()
                .map(|t| sole_table(t).ok_or("deletes through a join".to_string()))
                .collect(),
        },
        Statement::Merge(merge) => one(factor_name(&merge.table), "merges into a derived table"),
        Statement::Truncate(truncate) => Ok(truncate.table_names.iter().map(|t| &t.name).collect()),
        Statement::Drop {
            object_type: ObjectType::Table,
            names,
            ..
        } => Ok(names.iter().collect()),
        Statement::CreateTable(create) => create_table(create).map(|name| vec![name]),
        _ => Err(
            "is not a statement kind a mapped destination runs (ALTER, views, sessions and \
             engine statements can write past the mapped database)"
                .into(),
        ),
    }
}

/// A `CREATE TABLE` that stores its rows in the table it creates.
fn create_table(create: &CreateTable) -> Result<&ObjectName, String> {
    if create.external
        || create.iceberg
        || create.location.is_some()
        || create.partition_of.is_some()
        || create.like.is_some()
    {
        return Err("creates an external, iceberg, partition or LIKE table".into());
    }
    match engine(&create.table_options) {
        Some(engine) if !allowed_engine(&engine) => Err(format!(
            "creates a table with the {engine} engine, which can write another table"
        )),
        _ => Ok(&create.name),
    }
}

/// A plain table — not `t(args)`, which some dialects read as a function.
fn factor_name(factor: &TableFactor) -> Option<&ObjectName> {
    match factor {
        TableFactor::Table {
            name, args: None, ..
        } => Some(name),
        _ => None,
    }
}

fn sole_table(table: &TableWithJoins) -> Option<&ObjectName> {
    if table.joins.is_empty() {
        factor_name(&table.relation)
    } else {
        None
    }
}

/// The `ENGINE = …` name of a `CREATE TABLE`, when it has one.
fn engine(options: &CreateTableOptions) -> Option<String> {
    let options = match options {
        CreateTableOptions::None => return None,
        CreateTableOptions::With(o)
        | CreateTableOptions::Options(o)
        | CreateTableOptions::Plain(o)
        | CreateTableOptions::TableProperties(o) => o,
    };
    options.iter().find_map(|option| match option {
        SqlOption::NamedParenthesizedList(list)
            if list.key.value.eq_ignore_ascii_case("engine") =>
        {
            Some(
                list.name
                    .as_ref()
                    .map(|n| n.value.clone())
                    .unwrap_or_default(),
            )
        }
        _ => None,
    })
}

/// ClickHouse engines that store rows in the table itself: the MergeTree
/// family, `Memory`, and the `Log` family.
fn allowed_engine(engine: &str) -> bool {
    engine.ends_with("MergeTree") || matches!(engine, "Memory" | "Log" | "TinyLog" | "StripeLog")
}

/// A write nested in an allowed statement's sources — a data-modifying CTE,
/// `SELECT … INTO` — is held: sources may read anything, and write nothing.
#[derive(Default)]
struct NestedWrites {
    seen_top: bool,
}

impl Visitor for NestedWrites {
    type Break = String;

    fn pre_visit_statement(&mut self, _statement: &Statement) -> ControlFlow<String> {
        if std::mem::replace(&mut self.seen_top, true) {
            return ControlFlow::Break("writes in its sources (a nested statement)".to_string());
        }
        ControlFlow::Continue(())
    }

    /// `IDENTIFIER('db.schema.t')` and its kin name a table by a function
    /// the fence cannot read: held wherever it appears.
    fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<String> {
        if has_function_part(relation) {
            return ControlFlow::Break(format!(
                "names {relation} through a function, which the fence cannot read"
            ));
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<String> {
        match &*query.body {
            SetExpr::Select(select) if select.into.is_some() => {
                ControlFlow::Break("creates a table with SELECT … INTO".to_string())
            }
            _ => ControlFlow::Continue(()),
        }
    }
}

/// One name part: its value, and whether it was quoted.
struct Part {
    value: String,
    quoted: bool,
}

/// Hold a write target that is not the mapped database's.
fn check_target(fence: &DestinationFence, name: &ObjectName) -> Result<(), String> {
    if has_function_part(name) {
        return Err(format!(
            "writes {name}, named through a function the fence cannot read"
        ));
    }
    let parts = parts(name, fence.dialect);
    if parts.is_empty() || parts.iter().any(|p| p.value.is_empty()) {
        return Err("writes a table with no name the fence can read".into());
    }
    let shown = parts
        .iter()
        .map(|p| p.value.as_str())
        .collect::<Vec<_>>()
        .join(".");
    let Some(qualifier) = qualifier(fence.dialect, &parts) else {
        return Ok(());
    };
    let qualifier = qualifier.map_err(|why| format!("writes `{shown}`, {why}"))?;
    if fence
        .production
        .iter()
        .any(|p| p.eq_ignore_ascii_case(&qualifier.value))
    {
        return Err(format!(
            "writes `{shown}` in production's database `{}`",
            qualifier.value
        ));
    }
    if fence
        .mapped
        .iter()
        .any(|m| same_name(qualifier, m, fence.dialect))
    {
        return Ok(());
    }
    Err(format!(
        "writes `{shown}` in `{}`, which is not the mapped destination's own database",
        qualifier.value
    ))
}

/// The part of a written name that must be the mapped database. `None`: the
/// name stays inside the mapped connection's database as written.
fn qualifier(dialect: SqlDialect, parts: &[Part]) -> Option<Result<&Part, &'static str>> {
    let two_part_is_database = dialect == SqlDialect::CLICKHOUSE
        || matches!(dialect, SqlDialect::Other(_))
        || dialect == SqlDialect::BigQuery;
    match parts.len() {
        0 | 1 => None,
        2 if dialect == SqlDialect::DuckDb && same_name(&parts[0], "main", dialect) => None,
        2 if two_part_is_database || dialect == SqlDialect::DuckDb => Some(Ok(&parts[0])),
        2 => None,
        3 if dialect == SqlDialect::BigQuery => Some(Err(
            "through a project, which staging cannot confirm is not production's",
        )),
        3 if !two_part_is_database => Some(Ok(&parts[0])),
        _ => Some(Err("deeper than a table name")),
    }
}

/// A quoted part is compared exactly; an unquoted one without case — except
/// on ClickHouse, MySQL and BigQuery, whose database (dataset) names are
/// case-sensitive.
fn same_name(part: &Part, configured: &str, dialect: SqlDialect) -> bool {
    let case_sensitive = dialect == SqlDialect::CLICKHOUSE
        || dialect == SqlDialect::MYSQL
        || dialect == SqlDialect::BigQuery;
    if part.quoted || case_sensitive {
        part.value == configured
    } else {
        part.value.eq_ignore_ascii_case(configured)
    }
}

fn has_function_part(name: &ObjectName) -> bool {
    name.0
        .iter()
        .any(|part| matches!(part, ObjectNamePart::Function(_)))
}

/// MSSQL's `OUTPUT … INTO t` writes a second table from a DML statement.
fn outputs_into_a_table(statement: &Statement) -> bool {
    let output = match statement {
        Statement::Insert(insert) => &insert.output,
        Statement::Update(update) => &update.output,
        Statement::Delete(delete) => &delete.output,
        Statement::Merge(merge) => &merge.output,
        _ => return false,
    };
    matches!(
        output,
        Some(OutputClause::Output {
            into_table: Some(_),
            ..
        })
    )
}

/// A name's parts. On BigQuery a quoted part holding dots
/// (`` `project.dataset.table` ``) is split, so it counts as the path it is;
/// elsewhere a quoted `"a.b"` is one identifier, and stays one part.
fn parts(name: &ObjectName, dialect: SqlDialect) -> Vec<Part> {
    let split = dialect == SqlDialect::BigQuery;
    name.0
        .iter()
        .flat_map(|part| {
            let (text, quoted) = match part {
                ObjectNamePart::Identifier(ident) => {
                    (ident.value.clone(), ident.quote_style.is_some())
                }
                other => (other.to_string(), false),
            };
            let pieces: Vec<&str> = if split && quoted {
                text.split('.').collect()
            } else {
                vec![text.as_str()]
            };
            pieces
                .into_iter()
                .map(|value| Part {
                    value: value.to_string(),
                    quoted,
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn verb(statement: &Statement) -> String {
    statement
        .to_string()
        .split_whitespace()
        .next()
        .unwrap_or("UNKNOWN")
        .to_ascii_uppercase()
}

fn unclassified(reason: &str) -> HeldStatement {
    held(
        "UNCLASSIFIED",
        "",
        format!("could not be classified ({reason})"),
    )
}

fn held(verb: &str, table: &str, why: String) -> HeldStatement {
    HeldStatement {
        verb: verb.to_string(),
        table: table.to_string(),
        why,
    }
}

#[cfg(test)]
#[path = "destination_sql_tests.rs"]
mod tests;
