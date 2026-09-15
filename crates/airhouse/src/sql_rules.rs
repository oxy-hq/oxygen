//! What SQL a custom app may send to its own Airhouse schema.
//!
//! An app's facts live in one DuckLake schema, `app_<writer>`. Two fences keep
//! them there. Airhouse scopes the credential when it can (`write_schemas` on
//! the mint, enforced by `airhouse-server`'s `write_scope`), and the Oxy host
//! checks every statement with this module before sending it — the only fence
//! when Airhouse predates scoped credentials. So these rules mirror Airhouse's
//! own, and they fail closed: SQL this module cannot parse is refused.
//!
//! Reads may name any schema; the workspace's facts are readable by design. But
//! no table function beyond `range` / `generate_series` / `unnest`: the rest read
//! files or reach DuckLake's metadata (`postgres_execute`,
//! `ducklake_add_data_files`) and so write around every rule below. The same
//! goes for scalar functions that read files, the environment or session
//! settings (`read_text`, `read_blob`, `getenv`, `current_setting`): refused
//! wherever they appear, because nothing guarantees the Airhouse data plane runs
//! with `enable_external_access = false`.
//!
//! Writes must name their target `app_<writer>.<table>`. An unqualified name is
//! refused rather than resolved, because what it resolves to depends on session
//! state this module cannot see. `CASCADE`, `TEMP` objects, multi-table DML and
//! renames out of the schema are refused for the same reason: each reaches
//! something the target name does not.
//!
//! [`check`] returns the statements re-rendered from the tree it checked, and
//! callers send those, not the original text — so DuckDB's lexer never gets to
//! find a second statement sqlparser did not.
//!
//! DuckLake has no keys, `UNIQUE`, indexes or foreign keys, and a table that
//! declares one fails and leaves its writer inert, so [`Access::Ddl`] refuses
//! them before anything runs.

use std::fmt::Display;
use std::ops::ControlFlow;

use sqlparser::ast::{
    AlterTable, AlterTableOperation, CascadeOption, ColumnDef, ColumnOption, Delete, Expr,
    FromTable, Insert, ObjectName, ObjectNamePart, ObjectType, Query, RenameTableNameKind,
    SchemaName, SetExpr, Statement, TableConstraint, TableFactor, TableObject, TableWithJoins,
    Visit, Visitor,
};
use sqlparser::dialect::DuckDbDialect;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};

/// Table functions that compute rows and touch nothing.
const PURE_TABLE_FUNCTIONS: [&str; 3] = ["range", "generate_series", "unnest"];

/// Scalar function families that reach outside the rows: files, the process
/// environment, session settings (which hold secrets such as S3 keys) and the
/// engine's own catalogs. Matched on the function's unqualified name.
const IO_FUNCTION_PREFIXES: [&str; 9] = [
    "read_",
    "parquet_",
    "ducklake_",
    "postgres_",
    "duckdb_",
    "iceberg_",
    "delta_",
    "sqlite_",
    "mysql_",
];
const IO_FUNCTIONS: [&str; 6] = [
    "getenv",
    "glob",
    "sniff_csv",
    "current_setting",
    "query",
    "query_table",
];

/// What the caller may do with the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// One read-only statement (`ctx.airhouse.query`).
    Read,
    /// One statement: a read, or DML whose targets are in the schema
    /// (`ctx.airhouse.exec`).
    Write,
    /// A migration file: any number of statements, DDL and DML, every target in
    /// the schema.
    Ddl,
}

/// Why a statement was refused, in words the app author can act on.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Refused(pub String);

/// Check `sql` against `schema` and return its statements, each re-rendered
/// from the checked tree. Send those, not `sql`.
pub fn check(sql: &str, schema: &str, access: Access) -> Result<Vec<String>, Refused> {
    let dialect = DuckDbDialect {};
    let tokens = Tokenizer::new(&dialect, sql).tokenize().map_err(unparsed)?;
    if tokens
        .iter()
        .any(|t| matches!(t, Token::Word(w) if w.quote_style == Some('`')))
    {
        return Err(Refused(
            "backtick-quoted identifiers are not DuckDB syntax; quote names with double quotes"
                .into(),
        ));
    }
    let statements = Parser::parse_sql(&dialect, sql).map_err(unparsed)?;
    if statements.is_empty() {
        return Err(Refused("there is no statement to run".into()));
    }
    if access != Access::Ddl && statements.len() > 1 {
        return Err(Refused(format!(
            "{} statements in one call; send one statement per call",
            statements.len()
        )));
    }
    let mut checker = Checker {
        schema: schema.to_ascii_lowercase(),
        access,
    };
    for statement in &statements {
        if let ControlFlow::Break(refused) = statement.visit(&mut checker) {
            return Err(refused);
        }
    }
    Ok(statements.iter().map(ToString::to_string).collect())
}

fn unparsed(e: impl Display) -> Refused {
    Refused(format!(
        "could not parse this as DuckDB SQL ({e}); SQL an app sends to Airhouse must parse"
    ))
}

fn not_allowed(what: &str) -> Refused {
    Refused(format!("{what} is not allowed from an app"))
}

/// Visits every node that matters, nested ones included: a `DELETE` inside a
/// CTE is a write even when the statement around it is a `SELECT`, and a table
/// function anywhere in a read can write.
struct Checker {
    schema: String,
    access: Access,
}

impl Visitor for Checker {
    type Break = Refused;

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<Refused> {
        into_flow(self.statement(statement))
    }

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Refused> {
        if selects_into(&query.body) {
            return ControlFlow::Break(Refused(
                "SELECT … INTO creates a table; declare tables in an airhouseMigrations file"
                    .into(),
            ));
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, factor: &TableFactor) -> ControlFlow<Refused> {
        into_flow(table_factor(factor))
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Refused> {
        if let Expr::Function(function) = expr
            && is_io_function(&function.name)
        {
            return ControlFlow::Break(Refused(format!(
                "function {} is not allowed from an app: it reads files, the environment or \
                 session settings rather than rows",
                function.name
            )));
        }
        ControlFlow::Continue(())
    }
}

fn is_io_function(name: &ObjectName) -> bool {
    let Some(ObjectNamePart::Identifier(last)) = name.0.last() else {
        return true;
    };
    let name = last.value.to_ascii_lowercase();
    IO_FUNCTIONS.contains(&name.as_str())
        || IO_FUNCTION_PREFIXES.iter().any(|p| name.starts_with(p))
}

fn into_flow(result: Result<(), Refused>) -> ControlFlow<Refused> {
    match result {
        Ok(()) => ControlFlow::Continue(()),
        Err(refused) => ControlFlow::Break(refused),
    }
}

impl Checker {
    fn statement(&self, statement: &Statement) -> Result<(), Refused> {
        if is_read_only(statement) {
            return Ok(());
        }
        match statement {
            Statement::Insert(_)
            | Statement::Update(_)
            | Statement::Delete(_)
            | Statement::Merge(_)
            | Statement::Truncate(_) => self.dml(statement),
            Statement::CreateTable(_)
            | Statement::CreateView(_)
            | Statement::AlterTable(_)
            | Statement::AlterView { .. }
            | Statement::Drop { .. }
            | Statement::CreateSchema { .. } => self.ddl(statement),
            Statement::CreateIndex(_) => Err(ducklake("CREATE INDEX")),
            Statement::StartTransaction { .. }
            | Statement::Commit { .. }
            | Statement::Rollback { .. } => Err(Refused(
                "transaction control is not allowed: each migration file already runs in its own \
                 transaction, and each ctx.airhouse call is one statement"
                    .into(),
            )),
            other => Err(not_allowed(&format!(
                "{} statements",
                leading_keyword(other)
            ))),
        }
    }

    fn dml(&self, statement: &Statement) -> Result<(), Refused> {
        let verb = leading_keyword(statement);
        self.need(Access::Write, &verb)?;
        match statement {
            Statement::Insert(insert) => self.insert(insert, &verb),
            Statement::Update(update) if update.output.is_none() => {
                self.single_table(&update.table, &verb)
            }
            Statement::Delete(delete) => self.delete(delete, &verb),
            Statement::Merge(merge) => match &merge.table {
                TableFactor::Table {
                    name, args: None, ..
                } if merge.output.is_none() => self.target(name, &verb),
                _ => Err(not_allowed("MERGE into a non-table target or with OUTPUT")),
            },
            Statement::Truncate(truncate)
                if !matches!(truncate.cascade, Some(CascadeOption::Cascade)) =>
            {
                truncate
                    .table_names
                    .iter()
                    .try_for_each(|t| self.target(&t.name, &verb))
            }
            _ => Err(not_allowed(&format!("{verb} with OUTPUT or CASCADE"))),
        }
    }

    fn insert(&self, insert: &Insert, verb: &str) -> Result<(), Refused> {
        let multi_table = insert.multi_table_insert_type.is_some()
            || !insert.multi_table_into_clauses.is_empty()
            || !insert.multi_table_when_clauses.is_empty()
            || insert.multi_table_else_clause.is_some();
        if multi_table || insert.output.is_some() {
            return Err(not_allowed("INSERT with several targets or OUTPUT"));
        }
        match &insert.table {
            TableObject::TableName(name) => self.target(name, verb),
            _ => Err(not_allowed(&format!(
                "{verb} into anything but a named table"
            ))),
        }
    }

    fn delete(&self, delete: &Delete, verb: &str) -> Result<(), Refused> {
        if delete.output.is_some() {
            return Err(not_allowed("DELETE … OUTPUT"));
        }
        // USING is a read; the table deleted from must be exactly one.
        let (FromTable::WithFromKeyword(from) | FromTable::WithoutKeyword(from)) = &delete.from;
        match (delete.tables.as_slice(), from.as_slice()) {
            ([], [table]) => self.single_table(table, verb),
            _ => Err(not_allowed("a DELETE from several tables")),
        }
    }

    /// The one table an UPDATE or DELETE modifies: a plain name, no joins.
    fn single_table(&self, table: &TableWithJoins, verb: &str) -> Result<(), Refused> {
        match &table.relation {
            TableFactor::Table {
                name, args: None, ..
            } if table.joins.is_empty() => self.target(name, verb),
            _ => Err(not_allowed(&format!(
                "{verb} of a join or a non-table target"
            ))),
        }
    }

    fn ddl(&self, statement: &Statement) -> Result<(), Refused> {
        let verb = leading_keyword(statement);
        self.need(Access::Ddl, &verb)?;
        match statement {
            Statement::CreateTable(create) => {
                // A TEMP table lands in the connection's own catalog, which
                // outlives the session on a pooled connection.
                if create.temporary || create.partition_of.is_some() {
                    return Err(not_allowed("CREATE TEMPORARY TABLE or PARTITION OF"));
                }
                self.target(&create.name, &verb)?;
                no_keys(&create.columns, &create.constraints)
            }
            Statement::CreateView(view) if !view.temporary && view.to.is_none() => {
                self.target(&view.name, &verb)
            }
            Statement::CreateView(_) => Err(not_allowed("CREATE TEMPORARY VIEW or VIEW … TO")),
            Statement::AlterTable(alter) => self.alter_table(alter, &verb),
            Statement::AlterView { name, .. } => self.target(name, &verb),
            Statement::Drop {
                object_type,
                names,
                cascade,
                temporary,
                table,
                ..
            } => match object_type {
                // CASCADE can take dependents outside the schema with it.
                ObjectType::Table | ObjectType::View
                    if *cascade || *temporary || table.is_some() =>
                {
                    Err(not_allowed("DROP with CASCADE, TEMPORARY or ON"))
                }
                ObjectType::Table | ObjectType::View => {
                    names.iter().try_for_each(|n| self.target(n, "DROP"))
                }
                other => Err(not_allowed(&format!("DROP {other}"))),
            },
            Statement::CreateSchema {
                schema_name, clone, ..
            } => match schema_name {
                SchemaName::Simple(name)
                    if clone.is_none()
                        && idents(name).as_deref() == Some(&[self.schema.clone()]) =>
                {
                    Ok(())
                }
                _ => Err(Refused(format!(
                    "CREATE SCHEMA is allowed only for this app's own schema, {}",
                    self.schema
                ))),
            },
            _ => unreachable!("ddl() is only called for DDL"),
        }
    }

    fn alter_table(&self, alter: &AlterTable, verb: &str) -> Result<(), Refused> {
        self.target(&alter.name, verb)?;
        alter.operations.iter().try_for_each(|op| match op {
            AlterTableOperation::AddColumn { column_def, .. } => {
                no_keys(std::slice::from_ref(column_def), &[])
            }
            AlterTableOperation::DropColumn { .. }
            | AlterTableOperation::RenameColumn { .. }
            | AlterTableOperation::AlterColumn { .. } => Ok(()),
            AlterTableOperation::RenameTable {
                table_name: RenameTableNameKind::As(new_name) | RenameTableNameKind::To(new_name),
            } => {
                // An unqualified new name stays in the table's schema; a
                // qualified one must itself be in the app's schema.
                if matches!(new_name.0.as_slice(), [ObjectNamePart::Identifier(_)]) {
                    Ok(())
                } else {
                    self.target(new_name, "ALTER TABLE … RENAME TO")
                }
            }
            AlterTableOperation::AddConstraint { constraint, .. } => {
                Err(ducklake(&constraint.to_string()))
            }
            _ => Err(not_allowed("ALTER TABLE with this operation")),
        })
    }

    fn need(&self, at_least: Access, verb: &str) -> Result<(), Refused> {
        let allowed = match at_least {
            Access::Read => true,
            Access::Write => self.access != Access::Read,
            Access::Ddl => self.access == Access::Ddl,
        };
        if allowed {
            return Ok(());
        }
        Err(Refused(match (self.access, at_least) {
            (Access::Read, Access::Write) => format!(
                "{verb} is a write, and ctx.airhouse.query runs reads only — use \
                 ctx.airhouse.exec or ctx.airhouse.append"
            ),
            _ => format!(
                "{verb} changes the schema — declare it in an airhouseMigrations file, which \
                 runs once, at publish"
            ),
        }))
    }

    /// A write target: exactly `<schema>.<table>`.
    fn target(&self, name: &ObjectName, verb: &str) -> Result<(), Refused> {
        match idents(name).as_deref() {
            Some([schema, _]) if *schema == self.schema => Ok(()),
            Some([table]) => Err(Refused(format!(
                "{verb} {name}: name it {}.{table} — an unqualified name is refused because what \
                 it resolves to depends on the session",
                self.schema
            ))),
            _ => Err(Refused(format!(
                "{verb} {name}: this app may write only in its own schema, {}",
                self.schema
            ))),
        }
    }
}

/// Statements that only read. A statement nested in one (an `EXPLAIN
/// ANALYZE`'s body) is visited and classified on its own.
fn is_read_only(statement: &Statement) -> bool {
    matches!(
        statement,
        Statement::Query(_)
            | Statement::Explain { .. }
            | Statement::ExplainTable { .. }
            | Statement::ShowTables { .. }
            | Statement::ShowColumns { .. }
            | Statement::ShowViews { .. }
            | Statement::ShowSchemas { .. }
            | Statement::ShowDatabases { .. }
            | Statement::ShowCatalogs { .. }
            | Statement::ShowObjects(_)
            | Statement::ShowFunctions { .. }
            | Statement::ShowVariable { .. }
            | Statement::ShowVariables { .. }
    )
}

/// A FROM-clause item: tables, subqueries and joins read; a table function
/// may do anything, so only the pure ones pass.
fn table_factor(factor: &TableFactor) -> Result<(), Refused> {
    match factor {
        TableFactor::Table { args: None, .. }
        | TableFactor::Derived { .. }
        | TableFactor::NestedJoin { .. }
        | TableFactor::UNNEST { .. }
        | TableFactor::Pivot { .. }
        | TableFactor::Unpivot { .. }
        | TableFactor::MatchRecognize { .. } => Ok(()),
        TableFactor::Table { name, .. } | TableFactor::Function { name, .. }
            if is_pure_table_function(name) =>
        {
            Ok(())
        }
        TableFactor::Table { name, .. } | TableFactor::Function { name, .. } => {
            Err(Refused(format!(
                "table function {name} is not allowed from an app: table functions can read files \
                 or write around these rules (only {} are)",
                PURE_TABLE_FUNCTIONS.join(", ")
            )))
        }
        _ => Err(not_allowed("this FROM-clause construct")),
    }
}

fn is_pure_table_function(name: &ObjectName) -> bool {
    matches!(
        name.0.as_slice(),
        [ObjectNamePart::Identifier(ident)]
            if PURE_TABLE_FUNCTIONS.iter().any(|f| ident.value.eq_ignore_ascii_case(f))
    )
}

/// The name's parts, lowercased the way DuckDB compares identifiers (quoted or
/// not). `None` when a part is computed rather than named.
fn idents(name: &ObjectName) -> Option<Vec<String>> {
    name.0
        .iter()
        .map(|part| match part {
            ObjectNamePart::Identifier(ident) => Some(ident.value.to_ascii_lowercase()),
            _ => None,
        })
        .collect()
}

fn no_keys(columns: &[ColumnDef], constraints: &[TableConstraint]) -> Result<(), Refused> {
    for column in columns {
        for def in &column.options {
            if matches!(
                def.option,
                ColumnOption::PrimaryKey(_) | ColumnOption::Unique(_) | ColumnOption::ForeignKey(_)
            ) {
                return Err(ducklake(&format!("column {} {}", column.name, def.option)));
            }
        }
    }
    for constraint in constraints {
        if matches!(
            constraint,
            TableConstraint::PrimaryKey(_)
                | TableConstraint::Unique(_)
                | TableConstraint::ForeignKey(_)
        ) {
            return Err(ducklake(&constraint.to_string()));
        }
    }
    Ok(())
}

fn ducklake(what: &str) -> Refused {
    Refused(format!(
        "{what}: DuckLake has no primary keys, UNIQUE constraints, indexes or foreign keys, and \
         a table that declares one fails and leaves its writer inert. Give each fact the id its \
         source assigned and keep one row per id when reading (internal-docs/data-placement.md)"
    ))
}

fn selects_into(body: &SetExpr) -> bool {
    match body {
        SetExpr::Select(select) => select.into.is_some(),
        SetExpr::Query(query) => selects_into(&query.body),
        SetExpr::SetOperation { left, right, .. } => selects_into(left) || selects_into(right),
        _ => false,
    }
}

/// `INSERT`, `DROP`, `ATTACH` … — the statement's first word, for messages.
fn leading_keyword(statement: &Statement) -> String {
    statement
        .to_string()
        .split_whitespace()
        .next()
        .unwrap_or("this")
        .to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "app_store_ops";

    fn ok(sql: &str, access: Access) {
        if let Err(e) = check(sql, S, access) {
            panic!("expected {sql:?} to pass as {access:?}, got: {e}");
        }
    }

    fn refused(sql: &str, access: Access) -> String {
        match check(sql, S, access) {
            Ok(_) => panic!("expected {sql:?} to be refused as {access:?}"),
            Err(Refused(msg)) => msg,
        }
    }

    #[test]
    fn reads_may_name_any_schema() {
        ok(
            "SELECT * FROM toast_pos.orders o JOIN app_store_ops.visits v ON o.id = v.order_id",
            Access::Read,
        );
        ok("WITH x AS (SELECT 1) SELECT * FROM x", Access::Read);
        ok("SELECT * FROM range(10)", Access::Read);
    }

    #[test]
    fn a_read_call_refuses_writes_and_says_where_they_go() {
        let msg = refused("INSERT INTO app_store_ops.t VALUES (1)", Access::Read);
        assert!(msg.contains("ctx.airhouse.exec"), "{msg}");
    }

    #[test]
    fn table_functions_are_refused_even_in_a_read() {
        for sql in [
            "SELECT * FROM postgres_execute('__ducklake_metadata_lake', 'DELETE FROM x')",
            "SELECT * FROM read_parquet('s3://bucket/other/*.parquet')",
            "SELECT * FROM ducklake_add_data_files('lake', 'orders', 's3://bucket/x.parquet')",
        ] {
            let msg = refused(sql, Access::Read);
            assert!(msg.contains("table function"), "{sql}: {msg}");
        }
    }

    #[test]
    fn file_env_and_setting_functions_are_refused_wherever_they_appear() {
        for (sql, access) in [
            ("SELECT read_text('/etc/passwd')", Access::Read),
            ("SELECT read_blob('/proc/self/environ')", Access::Read),
            ("SELECT getenv('HOME')", Access::Read),
            (
                "SELECT current_setting('s3_secret_access_key')",
                Access::Read,
            ),
            (
                "INSERT INTO app_store_ops.notes SELECT read_text('/etc/hosts')",
                Access::Write,
            ),
        ] {
            let msg = refused(sql, access);
            assert!(msg.contains("reads files"), "{sql}: {msg}");
        }
        ok(
            "SELECT upper(note), count(*) FROM app_store_ops.visits GROUP BY 1",
            Access::Read,
        );
    }

    #[test]
    fn writes_land_only_in_the_apps_schema() {
        ok(
            "INSERT INTO app_store_ops.visits SELECT * FROM toast_pos.orders",
            Access::Write,
        );
        ok(
            "DELETE FROM app_store_ops.visits WHERE recorded_at < now() - INTERVAL 90 DAY",
            Access::Write,
        );
        ok(
            "UPDATE app_store_ops.visits SET note = 'x' WHERE id = 1",
            Access::Write,
        );
        let msg = refused("INSERT INTO toast_pos.orders VALUES (1)", Access::Write);
        assert!(msg.contains("only in its own schema"), "{msg}");
    }

    #[test]
    fn an_unqualified_target_is_refused_with_the_name_to_use() {
        let msg = refused("INSERT INTO visits VALUES (1)", Access::Write);
        assert!(msg.contains("app_store_ops.visits"), "{msg}");
    }

    #[test]
    fn quoting_and_case_do_not_open_another_schema() {
        ok(
            r#"INSERT INTO "APP_STORE_OPS"."Visits" VALUES (1)"#,
            Access::Write,
        );
        refused(r#"INSERT INTO "Other"."t" VALUES (1)"#, Access::Write);
        refused("INSERT INTO `app_store_ops`.`t` VALUES (1)", Access::Write);
    }

    #[test]
    fn a_catalog_qualified_target_is_refused() {
        refused(
            "INSERT INTO lake.app_store_ops.visits VALUES (1)",
            Access::Write,
        );
        refused(
            "INSERT INTO other_db.app_store_ops.visits VALUES (1)",
            Access::Write,
        );
    }

    #[test]
    fn a_write_hidden_in_a_cte_is_still_checked() {
        refused(
            "WITH gone AS (DELETE FROM toast_pos.orders RETURNING *) SELECT * FROM gone",
            Access::Read,
        );
    }

    #[test]
    fn one_statement_per_call_outside_migrations() {
        refused("SELECT 1; DROP TABLE app_store_ops.visits", Access::Write);
        refused("BEGIN; DROP TABLE t", Access::Read);
    }

    #[test]
    fn what_is_sent_is_what_was_checked() {
        let statements = check("select *   from app_store_ops.visits", S, Access::Read).unwrap();
        assert_eq!(
            statements,
            vec!["SELECT * FROM app_store_ops.visits".to_string()]
        );
    }

    #[test]
    fn engine_and_session_statements_are_refused() {
        for sql in [
            "ATTACH 'x.db' AS x",
            "SET search_path = 'toast_pos'",
            "COPY app_store_ops.visits TO 's3://bucket/x.parquet'",
            "CALL ducklake_expire_snapshots('lake')",
            "PRAGMA database_list",
        ] {
            refused(sql, Access::Ddl);
        }
    }

    #[test]
    fn garbage_is_refused_not_passed_through() {
        refused("SELEKT frm", Access::Read);
        refused("", Access::Read);
    }

    #[test]
    fn schema_changes_need_a_migration() {
        let msg = refused("CREATE TABLE app_store_ops.t (id VARCHAR)", Access::Write);
        assert!(msg.contains("airhouseMigrations"), "{msg}");
    }

    #[test]
    fn a_migration_may_create_and_alter_its_own_tables() {
        let statements = check(
            "CREATE SCHEMA IF NOT EXISTS app_store_ops;
             CREATE TABLE app_store_ops.visits (visit_id VARCHAR NOT NULL, recorded_at TIMESTAMPTZ);
             ALTER TABLE app_store_ops.visits ADD COLUMN note VARCHAR;
             ALTER TABLE app_store_ops.visits RENAME COLUMN note TO remark;
             CREATE VIEW app_store_ops.latest AS SELECT * FROM app_store_ops.visits;",
            S,
            Access::Ddl,
        )
        .expect("migration passes");
        assert_eq!(statements.len(), 5);
    }

    #[test]
    fn a_migration_may_not_touch_another_schema() {
        refused("CREATE TABLE toast_pos.t (id VARCHAR)", Access::Ddl);
        refused("CREATE SCHEMA toast_pos", Access::Ddl);
        refused("DROP TABLE toast_pos.orders", Access::Ddl);
        refused("DROP SCHEMA app_store_ops", Access::Ddl);
        refused(
            "ALTER TABLE app_store_ops.visits RENAME TO toast_pos.visits",
            Access::Ddl,
        );
    }

    #[test]
    fn cascade_and_temporary_objects_are_refused() {
        refused("DROP TABLE app_store_ops.visits CASCADE", Access::Ddl);
        refused(
            "CREATE TEMP TABLE app_store_ops.scratch (id VARCHAR)",
            Access::Ddl,
        );
    }

    #[test]
    fn keys_and_indexes_are_refused_before_ducklake_sees_them() {
        for sql in [
            "CREATE TABLE app_store_ops.t (id VARCHAR PRIMARY KEY)",
            "CREATE TABLE app_store_ops.t (id VARCHAR UNIQUE)",
            "CREATE TABLE app_store_ops.t (id VARCHAR, PRIMARY KEY (id))",
            "CREATE TABLE app_store_ops.t (id VARCHAR, UNIQUE (id))",
            "CREATE TABLE app_store_ops.t (o VARCHAR REFERENCES app_store_ops.o (id))",
            "CREATE INDEX i ON app_store_ops.t (id)",
            "ALTER TABLE app_store_ops.t ADD PRIMARY KEY (id)",
        ] {
            let msg = refused(sql, Access::Ddl);
            assert!(msg.contains("DuckLake"), "{sql}: {msg}");
        }
    }

    #[test]
    fn transaction_control_is_refused_inside_a_file() {
        refused(
            "BEGIN; CREATE TABLE app_store_ops.t (id VARCHAR); COMMIT;",
            Access::Ddl,
        );
    }
}
