//! The read half: every table a statement reads resolves to the preview's
//! copy when there is one, and stays live otherwise.

use std::ops::ControlFlow;

use sqlparser::ast::{Expr, ObjectName, Query, Statement, TableFactor, VisitMut, VisitorMut};

use super::names::{Name, table_factor};
use super::{PreviewNamespace, Refused, RewriteOptions, ShadowMap};
use crate::sql_parse::{address, check_relation, write_target_addresses};
use crate::sql_rules::{is_io_function, is_read_only, leading_keyword, selects_into};

/// Overlay `statement`'s reads and return the live tables redirected. Also
/// refuses what no preview statement may contain: a write nested in another
/// statement, `SELECT … INTO`, IO functions, unlisted table functions and
/// table names DuckDB could read as files.
pub(super) fn apply(
    ns: &PreviewNamespace,
    shadow: &ShadowMap,
    opts: &RewriteOptions,
    statement: &mut Statement,
) -> Result<Vec<(String, String)>, Refused> {
    let mut overlay = Overlay {
        ns,
        shadow,
        opts,
        ctes: Vec::new(),
        depth: 0,
        redirected: Vec::new(),
        write_targets: write_target_addresses(statement),
    };
    // `DESCRIBE S.t` and `SHOW COLUMNS FROM S.t` read a table outside any FROM.
    if let Some(table) = described_table_mut(statement) {
        check_relation(table)?;
        overlay.read(table)?;
    }
    match statement.visit(&mut overlay) {
        ControlFlow::Continue(()) => Ok(overlay.redirected),
        ControlFlow::Break(refused) => Err(refused),
    }
}

struct Overlay<'a> {
    ns: &'a PreviewNamespace,
    shadow: &'a ShadowMap,
    opts: &'a RewriteOptions,
    /// CTE names in scope, one frame per enclosing query.
    ctes: Vec<Vec<String>>,
    /// Statement nesting: 1 is the statement being overlaid.
    depth: usize,
    redirected: Vec<(String, String)>,
    /// Where the statement's DML target sits: written, so never a file read.
    write_targets: Vec<usize>,
}

impl VisitorMut for Overlay<'_> {
    type Break = Refused;

    fn pre_visit_statement(&mut self, statement: &mut Statement) -> ControlFlow<Refused> {
        self.depth += 1;
        if self.depth > 1 && !is_read_only(statement) {
            return ControlFlow::Break(Refused(format!(
                "{} nested in another statement is not allowed in a preview; write it as a \
                 statement of its own (INSERT INTO s.t WITH … SELECT …, not WITH … INSERT)",
                leading_keyword(statement)
            )));
        }
        ControlFlow::Continue(())
    }

    fn post_visit_statement(&mut self, _statement: &mut Statement) -> ControlFlow<Refused> {
        self.depth -= 1;
        ControlFlow::Continue(())
    }

    fn pre_visit_query(&mut self, query: &mut Query) -> ControlFlow<Refused> {
        if selects_into(&query.body) {
            return ControlFlow::Break(Refused(
                "SELECT … INTO creates a table; in a preview write CREATE TABLE s.t AS SELECT …"
                    .into(),
            ));
        }
        let names = query.with.iter().flat_map(|with| &with.cte_tables);
        let frame = names
            .map(|cte| cte.alias.name.value.to_ascii_lowercase())
            .collect();
        self.ctes.push(frame);
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, _query: &mut Query) -> ControlFlow<Refused> {
        self.ctes.pop();
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, factor: &mut TableFactor) -> ControlFlow<Refused> {
        if let TableFactor::Table { name, .. } = factor
            && !self.write_targets.contains(&address(name))
            && let Err(refused) = check_relation(name)
        {
            return ControlFlow::Break(refused);
        }
        flow(match factor {
            TableFactor::Table {
                name, args: None, ..
            } => self.read(name),
            other => table_factor(other),
        })
    }

    fn pre_visit_expr(&mut self, expr: &mut Expr) -> ControlFlow<Refused> {
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

/// The table `DESCRIBE` or `SHOW COLUMNS FROM` reads, to redirect.
fn described_table_mut(statement: &mut Statement) -> Option<&mut ObjectName> {
    match statement {
        Statement::ExplainTable { table_name, .. } => Some(table_name),
        Statement::ShowColumns { show_options, .. } => show_options
            .show_in
            .as_mut()
            .and_then(|show_in| show_in.parent_name.as_mut()),
        _ => None,
    }
}

fn flow(result: Result<(), Refused>) -> ControlFlow<Refused> {
    match result {
        Ok(()) => ControlFlow::Continue(()),
        Err(refused) => ControlFlow::Break(refused),
    }
}

impl Overlay<'_> {
    /// Resolve one table read, redirecting it when the preview has a copy.
    fn read(&mut self, name: &mut ObjectName) -> Result<(), Refused> {
        let resolved = Name::resolve(name, "a read of", self.opts)?;
        if resolved.unqualified().is_some_and(|t| self.in_cte_scope(t)) {
            return Ok(());
        }
        let live = resolved.live();
        if self.ns.owns_schema(&live.0) {
            return Ok(());
        }
        let preview_schema = self.ns.schema_for(&live.0)?;
        if self.opts.read_live_only || self.shadow.state(&live).is_none() {
            return Ok(());
        }
        *name = resolved.in_schema(&preview_schema);
        if !self.redirected.contains(&live) {
            self.redirected.push(live);
        }
        Ok(())
    }

    fn in_cte_scope(&self, table: &str) -> bool {
        let table = table.to_ascii_lowercase();
        self.ctes.iter().flatten().any(|cte| *cte == table)
    }
}
