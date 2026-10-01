//! The independent fence: nothing the preview connector sends may write
//! outside the preview's schemas, whether or not [`super::rewrite`] saw it.
//!
//! Deliberately shares no decision with the rewrite beyond parsing and the
//! function lists: it does not map names, it only checks that every table,
//! view or schema a statement writes (nested statements included) is one the
//! namespace owns, in the workspace's catalog when a catalog is named.

use std::cell::RefCell;
use std::ops::ControlFlow;

use sqlparser::ast::{
    AlterTableOperation, Delete, Expr, FromTable, Insert, ObjectName, ObjectType, Query,
    RenameTableNameKind, SchemaName, Statement, TableFactor, TableObject, TableWithJoins, Visit,
    Visitor,
};

use super::names::table_factor;
use super::{PreviewNamespace, Refused, StatementRole, Verified, transactions};
use crate::sql_parse::{address, check_relation, described_table, write_target_addresses};
use crate::sql_rules::{idents, is_io_function, is_read_only, leading_keyword, selects_into};

/// Call inside `on_sql_stack`, which parsed `statements` and drops them: the
/// tree is walked and rendered here. Every statement is checked before any is
/// returned, so one refusal refuses the batch.
pub(super) fn verify(
    statements: &[Statement],
    ns: &PreviewNamespace,
    catalog: Option<&str>,
) -> Result<Vec<Verified>, Refused> {
    transactions::check(statements)?;
    let mut verifier = Verifier {
        ns,
        catalog: catalog.map(str::to_ascii_lowercase),
        write_targets: Vec::new(),
        written: RefCell::new(Vec::new()),
        relations: RefCell::new(Vec::new()),
    };
    let mut verified = Vec::with_capacity(statements.len());
    for statement in statements {
        if let ControlFlow::Break(refused) = statement.visit(&mut verifier) {
            return Err(refused);
        }
        let mut relations = verifier.relations.take();
        relations.sort();
        relations.dedup();
        verified.push(Verified {
            sql: statement.to_string(),
            role: role(statement, verifier.written.take()),
            relations,
        });
    }
    Ok(verified)
}

/// What a checked statement does, from the preview schemas it was seen to
/// write (nested statements included).
fn role(statement: &Statement, mut written: Vec<String>) -> StatementRole {
    written.sort();
    written.dedup();
    match statement {
        Statement::StartTransaction { .. } => StatementRole::Begin,
        Statement::Commit { .. } | Statement::Rollback { .. } => StatementRole::End,
        Statement::CreateSchema { .. }
        | Statement::Drop {
            object_type: ObjectType::Schema,
            ..
        } => StatementRole::SchemaDdl(written),
        _ if written.is_empty() => StatementRole::Read,
        _ => StatementRole::Write(written),
    }
}

struct Verifier<'a> {
    ns: &'a PreviewNamespace,
    /// The workspace's catalog, lowercase: the only one a name may give.
    catalog: Option<String>,
    /// Where the DML targets seen so far sit: written, never read as files.
    write_targets: Vec<usize>,
    /// The preview schemas the statement being visited writes, lowercase.
    written: RefCell<Vec<String>>,
    /// The relations it writes, `(preview schema, relation)`, lowercase.
    relations: RefCell<Vec<(String, String)>>,
}

impl Visitor for Verifier<'_> {
    type Break = Refused;

    fn pre_visit_statement(&mut self, statement: &Statement) -> ControlFlow<Refused> {
        self.write_targets.extend(write_target_addresses(statement));
        if let Some(table) = described_table(statement)
            && let Err(refused) = check_relation(table)
        {
            return ControlFlow::Break(refused);
        }
        flow(self.statement(statement))
    }

    fn pre_visit_query(&mut self, query: &Query) -> ControlFlow<Refused> {
        if selects_into(&query.body) {
            return ControlFlow::Break(Refused(
                "SELECT … INTO creates a table outside the preview's schemas".into(),
            ));
        }
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, factor: &TableFactor) -> ControlFlow<Refused> {
        if let TableFactor::Table { name, .. } = factor
            && !self.write_targets.contains(&address(name))
            && let Err(refused) = check_relation(name)
        {
            return ControlFlow::Break(refused);
        }
        flow(table_factor(factor))
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> ControlFlow<Refused> {
        if let Expr::Function(function) = expr
            && is_io_function(&function.name)
        {
            return ControlFlow::Break(Refused(format!(
                "function {} is not allowed in a preview: it reads files, the environment or \
                 session settings, or advances a sequence, rather than reading rows",
                function.name
            )));
        }
        ControlFlow::Continue(())
    }
}

fn flow(result: Result<(), Refused>) -> ControlFlow<Refused> {
    match result {
        Ok(()) => ControlFlow::Continue(()),
        Err(refused) => ControlFlow::Break(refused),
    }
}

impl Verifier<'_> {
    /// Reads pass; each kind of write names its targets, and every one must
    /// be the preview's; every other kind of statement is refused.
    fn statement(&self, statement: &Statement) -> Result<(), Refused> {
        // `EXPLAIN ANALYZE` runs its body, and a statement's role is read off
        // the outer statement only, so a wrapped write or schema DDL would be
        // sent as a read. Only a query may be explained.
        if let Statement::Explain {
            statement: body, ..
        } = statement
            && !matches!(**body, Statement::Query(_))
        {
            return Err(Refused(format!(
                "EXPLAIN of {} is refused in a preview: EXPLAIN ANALYZE runs what it explains, \
                 so only a query may be explained",
                leading_keyword(body)
            )));
        }
        if is_read_only(statement) {
            return Ok(());
        }
        match statement {
            Statement::Insert(insert) => self.insert(insert),
            Statement::Update(update) => self.tables(std::slice::from_ref(&update.table)),
            Statement::Delete(delete) => self.delete(delete),
            Statement::Merge(merge) => self.factor(&merge.table),
            Statement::CreateTable(create) => self.table(&create.name),
            Statement::CreateView(view) => {
                self.table(&view.name)?;
                view.to.iter().try_for_each(|to| self.table(to))
            }
            Statement::AlterTable(alter) => {
                self.table(&alter.name)?;
                alter
                    .operations
                    .iter()
                    .try_for_each(|op| self.alter(op, &alter.name))
            }
            Statement::Truncate(truncate) => truncate
                .table_names
                .iter()
                .try_for_each(|t| self.table(&t.name)),
            Statement::Drop {
                object_type, names, ..
            } => match object_type {
                ObjectType::Table | ObjectType::View => {
                    names.iter().try_for_each(|n| self.table(n))
                }
                ObjectType::Schema => names.iter().try_for_each(|n| self.schema(n)),
                other => Err(outside(&format!("DROP {other}"))),
            },
            Statement::CreateSchema {
                schema_name: SchemaName::Simple(name),
                ..
            } => self.schema(name),
            // Balanced by `transactions::check` before any statement is visited.
            Statement::StartTransaction { .. }
            | Statement::Commit { .. }
            | Statement::Rollback { .. } => Ok(()),
            other => Err(Refused(format!(
                "{} statements are not allowed in a preview",
                leading_keyword(other)
            ))),
        }
    }

    fn insert(&self, insert: &Insert) -> Result<(), Refused> {
        let multi_table = insert.multi_table_insert_type.is_some()
            || !insert.multi_table_into_clauses.is_empty()
            || !insert.multi_table_when_clauses.is_empty()
            || insert.multi_table_else_clause.is_some();
        match &insert.table {
            TableObject::TableName(name) if !multi_table => self.table(name),
            _ => Err(outside(
                "an INSERT into several tables or a non-table target",
            )),
        }
    }

    /// `DELETE t1 FROM …` deletes from `t1`; otherwise from the FROM tables.
    fn delete(&self, delete: &Delete) -> Result<(), Refused> {
        if !delete.tables.is_empty() {
            return delete.tables.iter().try_for_each(|n| self.table(n));
        }
        let (FromTable::WithFromKeyword(from) | FromTable::WithoutKeyword(from)) = &delete.from;
        self.tables(from)
    }

    /// Every table in a DML target, joined ones included.
    fn tables(&self, targets: &[TableWithJoins]) -> Result<(), Refused> {
        for target in targets {
            self.factor(&target.relation)?;
            for join in &target.joins {
                self.factor(&join.relation)?;
            }
        }
        Ok(())
    }

    fn factor(&self, factor: &TableFactor) -> Result<(), Refused> {
        match factor {
            TableFactor::Table {
                name, args: None, ..
            } => self.table(name),
            _ => Err(outside("a write to anything but a named table")),
        }
    }

    fn alter(&self, op: &AlterTableOperation, table: &ObjectName) -> Result<(), Refused> {
        match op {
            AlterTableOperation::AddColumn { .. }
            | AlterTableOperation::DropColumn { .. }
            | AlterTableOperation::RenameColumn { .. }
            | AlterTableOperation::AlterColumn { .. } => Ok(()),
            AlterTableOperation::RenameTable {
                table_name: RenameTableNameKind::As(new) | RenameTableNameKind::To(new),
            } => match (idents(new).as_deref(), idents(table).as_deref()) {
                // An unqualified new name stays in the table's own schema,
                // which `table` already checked.
                (Some([new]), Some([.., schema, _])) => {
                    self.relations
                        .borrow_mut()
                        .push((schema.clone(), new.clone()));
                    Ok(())
                }
                _ => self.table(new),
            },
            _ => Err(outside("ALTER TABLE with this operation")),
        }
    }

    /// A written table: `schema.table`, or `catalog.schema.table` in the
    /// workspace's catalog, the schema one of the preview's.
    fn table(&self, name: &ObjectName) -> Result<(), Refused> {
        match idents(name).as_deref() {
            Some([schema, table]) if self.ns.owns_schema(schema) => self.relation(schema, table),
            Some([catalog, schema, table]) if self.ours(catalog) && self.ns.owns_schema(schema) => {
                self.relation(schema, table)
            }
            Some([_]) => Err(Refused(format!(
                "a write to {name}, an unqualified name, is refused in a preview: what it \
                 resolves to depends on the session"
            ))),
            _ => Err(outside(&format!("a write to {name}"))),
        }
    }

    /// A created or dropped schema: the preview's, in the workspace's catalog
    /// when one is named.
    fn schema(&self, name: &ObjectName) -> Result<(), Refused> {
        match idents(name).as_deref() {
            Some([schema]) if self.ns.owns_schema(schema) => self.writes(schema),
            Some([catalog, schema]) if self.ours(catalog) && self.ns.owns_schema(schema) => {
                self.writes(schema)
            }
            _ => Err(outside(&format!("schema {name}"))),
        }
    }

    fn ours(&self, catalog: &str) -> bool {
        self.catalog.as_deref() == Some(catalog)
    }

    /// Record that the statement being visited writes `schema`, one the
    /// preview owns ([`StatementRole::Write`]).
    fn writes(&self, schema: &str) -> Result<(), Refused> {
        self.written.borrow_mut().push(schema.to_string());
        Ok(())
    }

    /// [`Self::writes`], and the relation it writes there
    /// ([`Verified::relations`]).
    fn relation(&self, schema: &str, relation: &str) -> Result<(), Refused> {
        self.relations
            .borrow_mut()
            .push((schema.to_string(), relation.to_string()));
        self.writes(schema)
    }
}

fn outside(what: &str) -> Refused {
    Refused(format!(
        "{what} is outside this preview's schemas (preview_<key>__…) in the workspace's catalog; \
         a preview writes nothing else"
    ))
}
