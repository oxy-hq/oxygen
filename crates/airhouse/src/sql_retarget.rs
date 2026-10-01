//! An app's statement, moved from its own schema to a sibling of it.
//!
//! A non-production environment writes to `app_<writer>__<env>`
//! ([`crate::app_schema`]) while the app's code names `app_<writer>`, exactly
//! as it does in production. [`check_retargeted`] is what lets the same code
//! run in both:
//!
//! 1. the statement is checked against the app's own schema — the rules
//!    production applies ([`sql_rules::check`]), so an environment refuses
//!    exactly what production refuses;
//! 2. every reference to that schema is renamed to the sibling, on the tree:
//!    table names wherever they appear, the names DDL creates, alters and
//!    drops, `CREATE SCHEMA`, and a column qualified by schema and table;
//! 3. the result is checked again against the **sibling**, and that check is
//!    the fence. A write target the rename missed still names `app_<writer>`
//!    and is refused there, so the rename can only fall short towards a
//!    refusal, never towards writing production.
//!
//! Reads inside the statement move with it — `INSERT INTO app_x.t SELECT …
//! FROM app_x.u` reads the sibling's `u` — so a statement never mixes the two.
//! A read of any other schema is left alone.

use std::ops::ControlFlow;

use sqlparser::ast::{
    AlterTableOperation, Expr, Ident, ObjectName, ObjectNamePart, RenameTableNameKind, SchemaName,
    Statement, VisitMut, VisitorMut,
};

use crate::sql_parse::on_sql_stack;
use crate::sql_rules::{self, Access, Refused};

/// Check `sql` as a statement for `from_schema`, then return it moved to
/// `to_schema`, checked there too. Each statement is re-rendered from its
/// tree, as [`sql_rules::check`] returns them. Send those, not `sql`.
pub fn check_retargeted(
    sql: &str,
    from_schema: &str,
    to_schema: &str,
    access: Access,
) -> Result<Vec<String>, Refused> {
    let as_written = sql_rules::check(sql, from_schema, access)?;
    // Each statement is the text `retarget_one` parses, so it is what decides
    // whether that parse needs the big stack.
    let moved = as_written
        .iter()
        .map(|statement| {
            on_sql_stack(statement, "an app", |statements| {
                retarget_one(statements, from_schema, to_schema)
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    moved
        .iter()
        .map(|statement| one_statement(sql_rules::check(statement, to_schema, access)?))
        .collect()
}

/// One statement, parsed again and renamed. Call it inside [`on_sql_stack`],
/// which parsed `statements`.
fn retarget_one(mut statements: Vec<Statement>, from: &str, to: &str) -> Result<String, Refused> {
    let mut rename = Rename {
        from: from.to_ascii_lowercase(),
        to,
    };
    for statement in &mut statements {
        let _ = statement.visit(&mut rename);
    }
    Ok(statements
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; "))
}

fn one_statement(mut statements: Vec<String>) -> Result<String, Refused> {
    match statements.len() {
        1 => Ok(statements.remove(0)),
        n => Err(Refused(format!(
            "a retargeted statement became {n}; send one statement per call"
        ))),
    }
}

/// Renames every reference to `from` (compared as DuckDB compares
/// identifiers) to `to`.
struct Rename<'a> {
    from: String,
    to: &'a str,
}

impl Rename<'_> {
    fn ident(&self, ident: &mut Ident) {
        if ident.value.eq_ignore_ascii_case(&self.from) {
            ident.value = self.to.to_string();
        }
    }

    /// `schema.table` or `catalog.schema.table`: the schema is the part before
    /// the last.
    fn name(&self, name: &mut ObjectName) {
        let len = name.0.len();
        if len >= 2
            && let ObjectNamePart::Identifier(schema) = &mut name.0[len - 2]
        {
            self.ident(schema);
        }
    }

    /// `CREATE SCHEMA app_x` names the schema alone.
    fn schema(&self, name: &mut ObjectName) {
        if let [ObjectNamePart::Identifier(schema)] = name.0.as_mut_slice() {
            self.ident(schema);
        }
    }
}

impl VisitorMut for Rename<'_> {
    type Break = ();

    fn pre_visit_relation(&mut self, relation: &mut ObjectName) -> ControlFlow<()> {
        self.name(relation);
        ControlFlow::Continue(())
    }

    /// Names sqlparser does not visit as relations.
    fn pre_visit_statement(&mut self, statement: &mut Statement) -> ControlFlow<()> {
        match statement {
            Statement::CreateView(view) => self.name(&mut view.name),
            Statement::Drop { names, .. } => names.iter_mut().for_each(|n| self.name(n)),
            Statement::CreateSchema {
                schema_name: SchemaName::Simple(name) | SchemaName::NamedAuthorization(name, _),
                ..
            } => self.schema(name),
            Statement::AlterTable(alter) => {
                for op in &mut alter.operations {
                    if let AlterTableOperation::RenameTable {
                        table_name: RenameTableNameKind::As(n) | RenameTableNameKind::To(n),
                    } = op
                    {
                        self.name(n);
                    }
                }
            }
            _ => {}
        }
        ControlFlow::Continue(())
    }

    /// `app_x.visits.visit_id`: a column qualified by schema and table.
    fn pre_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<()> {
        if let Expr::CompoundIdentifier(parts) = expr {
            let len = parts.len();
            if len >= 3 {
                self.ident(&mut parts[len - 3]);
            }
        }
        ControlFlow::Continue(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROD: &str = "app_store_ops";
    const STAGING: &str = "app_store_ops__staging";

    fn moved(sql: &str, access: Access) -> Vec<String> {
        check_retargeted(sql, PROD, STAGING, access)
            .unwrap_or_else(|e| panic!("expected {sql:?} to move, got: {e}"))
    }

    #[test]
    fn a_write_and_the_reads_it_makes_move_to_the_sibling() {
        let sql = "INSERT INTO app_store_ops.daily SELECT v.store, count(*) \
                   FROM app_store_ops.visits AS v JOIN main.stores AS s ON s.id = v.store \
                   GROUP BY v.store";
        let got = moved(sql, Access::Write).join("\n");
        assert!(got.contains("app_store_ops__staging.daily"), "{got}");
        assert!(got.contains("app_store_ops__staging.visits"), "{got}");
        assert!(
            got.contains("main.stores"),
            "another schema is left alone: {got}"
        );
        assert!(
            !got.replace("app_store_ops__staging", "")
                .contains("app_store_ops"),
            "nothing still names production: {got}"
        );
    }

    #[test]
    fn update_delete_and_quoted_names_move() {
        for sql in [
            "UPDATE app_store_ops.visits SET n = n + 1 WHERE id = 'a'",
            "DELETE FROM app_store_ops.visits WHERE id = 'a'",
            r#"INSERT INTO "app_store_ops"."visits" ("id") VALUES ('it''s')"#,
            "INSERT INTO APP_STORE_OPS.visits (id) VALUES ('a')",
        ] {
            let got = moved(sql, Access::Write).remove(0);
            assert!(
                got.to_ascii_lowercase().contains("app_store_ops__staging"),
                "{sql} → {got}"
            );
        }
    }

    #[test]
    fn a_literal_holding_the_schema_name_is_data_not_a_name() {
        let got = moved(
            "INSERT INTO app_store_ops.notes (body) VALUES ('see app_store_ops.visits')",
            Access::Write,
        )
        .remove(0);
        assert!(got.contains("'see app_store_ops.visits'"), "{got}");
    }

    #[test]
    fn a_migration_file_moves_whole() {
        let sql = "CREATE SCHEMA IF NOT EXISTS app_store_ops;
                   CREATE TABLE app_store_ops.visits (id VARCHAR NOT NULL, at TIMESTAMPTZ);
                   CREATE VIEW app_store_ops.latest AS SELECT app_store_ops.visits.id FROM app_store_ops.visits;
                   ALTER TABLE app_store_ops.visits ADD COLUMN n INTEGER;
                   DROP VIEW app_store_ops.latest;";
        let got = moved(sql, Access::Ddl);
        assert_eq!(got.len(), 5);
        for statement in &got {
            assert!(
                !statement
                    .replace("app_store_ops__staging", "")
                    .contains("app_store_ops"),
                "{statement}"
            );
        }
    }

    /// What production refuses is refused before anything moves, with
    /// production's reason.
    #[test]
    fn production_refusals_stand() {
        for (sql, access) in [
            ("INSERT INTO other_app.t VALUES (1)", Access::Write),
            ("INSERT INTO visits VALUES (1)", Access::Write),
            (
                "CREATE TABLE app_store_ops.t (id VARCHAR PRIMARY KEY)",
                Access::Ddl,
            ),
            ("CREATE TABLE app_store_ops.t (id VARCHAR)", Access::Write),
            // Naming the sibling directly is not the app's own schema.
            (
                "INSERT INTO app_store_ops__staging.t VALUES (1)",
                Access::Write,
            ),
        ] {
            assert!(
                check_retargeted(sql, PROD, STAGING, access).is_err(),
                "{sql} must be refused"
            );
        }
    }

    #[test]
    fn a_read_only_statement_moves_too() {
        let got = moved("SELECT count(*) FROM app_store_ops.visits", Access::Write).remove(0);
        assert!(got.contains("app_store_ops__staging.visits"), "{got}");
    }
}
