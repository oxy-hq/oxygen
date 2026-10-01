//! The write half: each statement's targets move into the preview.

use std::collections::HashMap;

use sqlparser::ast::{
    AlterTable, AlterTableOperation, CreateTable, CreateView, Delete, FromTable, Ident, Insert,
    Merge, ObjectName, RenameTableNameKind, Statement, TableFactor, TableObject, TableWithJoins,
    Update,
};

use super::names::{self, Target, plain_parts};
use super::{
    Prelude, PreviewNamespace, Refused, Rewrite, RewriteOptions, ShadowMap, ShadowState, overlay,
};
use crate::sql_rules::{is_read_only, leading_keyword};

/// What a statement leaves the preview holding for one table.
pub(super) enum Effect {
    Set((String, String), ShadowState),
    /// The state of `source`'s copy when the host made one, else `fallback`.
    FromCopy {
        live: (String, String),
        source: (String, String),
        fallback: ShadowState,
    },
}

pub(super) struct Rewriter<'a> {
    pub(super) ns: &'a PreviewNamespace,
    pub(super) opts: &'a RewriteOptions,
    /// The preview as of the statement being rewritten. A table this rewrite
    /// queued a copy of reads as `Shadow` here from the copy on.
    pub(super) shadow: ShadowMap,
    /// Tables whose state is their copy's, which the host settles: table →
    /// the live table copied.
    pending: HashMap<(String, String), (String, String)>,
    pub(super) sent: Vec<String>,
    out: Rewrite,
}

impl<'a> Rewriter<'a> {
    pub fn new(ns: &'a PreviewNamespace, shadow: ShadowMap, opts: &'a RewriteOptions) -> Self {
        Self {
            ns,
            opts,
            shadow,
            pending: HashMap::new(),
            sent: Vec::new(),
            out: Rewrite::default(),
        }
    }

    pub fn finish(mut self) -> Rewrite {
        self.out.sql = self.sent.join(";\n");
        self.out
    }

    /// Map the statement's targets (queuing any copy first), overlay its
    /// reads — which see a copy made for this very statement — then record
    /// what it leaves behind.
    pub fn statement(&mut self, mut statement: Statement) -> Result<(), Refused> {
        // These become other statements, or none, and read nothing to overlay.
        match &statement {
            Statement::CreateSchema { .. } => return self.create_schema(&statement),
            Statement::Drop { .. } => return self.drop(&statement),
            Statement::Truncate(truncate) => return self.truncate(truncate),
            _ => {}
        }
        let verb = leading_keyword(&statement);
        let effects = match &mut statement {
            s if is_read_only(s) => Vec::new(),
            Statement::Insert(insert) => self.insert(insert, &verb)?,
            Statement::Update(update) => self.update(update, &verb)?,
            Statement::Delete(delete) => self.delete(delete, &verb)?,
            Statement::Merge(merge) => self.merge(merge, &verb)?,
            Statement::CreateTable(create) => self.create_table(create, &verb)?,
            Statement::CreateView(view) => self.create_view(view, &verb)?,
            Statement::AlterTable(alter) => self.alter_table(alter, &verb)?,
            // Balanced by `transactions::check` before the first statement.
            Statement::StartTransaction { .. }
            | Statement::Commit { .. }
            | Statement::Rollback { .. } => Vec::new(),
            _ => {
                return Err(Refused(format!(
                    "{verb} statements are not allowed in a preview"
                )));
            }
        };
        let reads = overlay::apply(self.ns, &self.shadow, self.opts, &mut statement)?;
        for live in reads {
            push_unique(&mut self.out.redirected_reads, live);
        }
        self.sent.push(statement.to_string());
        effects.into_iter().for_each(|effect| self.record(effect));
        Ok(())
    }

    fn insert(&mut self, insert: &mut Insert, verb: &str) -> Result<Vec<Effect>, Refused> {
        let multi_table = insert.multi_table_insert_type.is_some()
            || !insert.multi_table_into_clauses.is_empty()
            || !insert.multi_table_when_clauses.is_empty()
            || insert.multi_table_else_clause.is_some();
        if multi_table || insert.output.is_some() {
            return Err(not_allowed("INSERT with several targets or OUTPUT"));
        }
        let TableObject::TableName(name) = &mut insert.table else {
            return Err(not_allowed(&format!(
                "{verb} into anything but a named table"
            )));
        };
        self.write_in_place(name, verb)
    }

    fn update(&mut self, update: &mut Update, verb: &str) -> Result<Vec<Effect>, Refused> {
        if update.output.is_some() {
            return Err(not_allowed("UPDATE … OUTPUT"));
        }
        let name = single_table(&mut update.table, verb)?;
        self.write_in_place(name, verb)
    }

    fn delete(&mut self, delete: &mut Delete, verb: &str) -> Result<Vec<Effect>, Refused> {
        if delete.output.is_some() || !delete.tables.is_empty() {
            return Err(not_allowed("DELETE from several tables or with OUTPUT"));
        }
        // USING is a read; the table deleted from must be exactly one.
        let (FromTable::WithFromKeyword(from) | FromTable::WithoutKeyword(from)) = &mut delete.from;
        let [table] = from.as_mut_slice() else {
            return Err(not_allowed("DELETE from several tables or with OUTPUT"));
        };
        let name = single_table(table, verb)?;
        self.write_in_place(name, verb)
    }

    fn merge(&mut self, merge: &mut Merge, verb: &str) -> Result<Vec<Effect>, Refused> {
        match &mut merge.table {
            TableFactor::Table {
                name, args: None, ..
            } if merge.output.is_none() => self.write_in_place(name, verb),
            _ => Err(not_allowed("MERGE into a non-table target or with OUTPUT")),
        }
    }

    /// DML: the write starts from the live table, so a table the preview has
    /// no copy of is copied first. The copy's state is the table's.
    fn write_in_place(
        &mut self,
        name: &mut ObjectName,
        verb: &str,
    ) -> Result<Vec<Effect>, Refused> {
        let target = self.target(name, verb)?;
        self.copy_on_write(&target);
        Ok(Vec::new())
    }

    fn create_table(
        &mut self,
        create: &mut CreateTable,
        verb: &str,
    ) -> Result<Vec<Effect>, Refused> {
        if create.temporary
            || create.partition_of.is_some()
            || create.like.is_some()
            || create.clone.is_some()
            || create.inherits.is_some()
        {
            return Err(not_allowed(
                "CREATE TEMPORARY TABLE, or CREATE TABLE … LIKE, CLONE, INHERITS or PARTITION OF",
            ));
        }
        let target = self.target(&mut create.name, verb)?;
        let before = self.shadow.state(&target.live);
        if create.or_replace {
            return Ok(vec![Effect::Set(target.live, ShadowState::Shadow)]);
        }
        // Without OR REPLACE the outcome depends on whether the table exists
        // (IF NOT EXISTS keeps it; plain CREATE fails): copy it, so it does.
        self.copy_on_write(&target);
        if create.if_not_exists && before.is_some_and(|s| s != ShadowState::Dropped) {
            return Ok(Vec::new());
        }
        Ok(vec![match self.pending.get(&target.live) {
            // Kept (or refused) when the copy was made; created when not.
            Some(source) => Effect::FromCopy {
                live: target.live.clone(),
                source: source.clone(),
                fallback: ShadowState::Shadow,
            },
            None => Effect::Set(target.live, ShadowState::Shadow),
        }])
    }

    fn create_view(&mut self, view: &mut CreateView, verb: &str) -> Result<Vec<Effect>, Refused> {
        if view.temporary || view.materialized || view.to.is_some() {
            return Err(not_allowed(
                "CREATE TEMPORARY or MATERIALIZED VIEW, or VIEW … TO",
            ));
        }
        let target = self.target(&mut view.name, verb)?;
        view.or_replace = true;
        view.if_not_exists = false;
        Ok(vec![Effect::Set(target.live, ShadowState::Shadow)])
    }

    fn alter_table(&mut self, alter: &mut AlterTable, verb: &str) -> Result<Vec<Effect>, Refused> {
        let target = self.target(&mut alter.name, verb)?;
        self.copy_on_write(&target);
        let mut effects = Vec::new();
        for op in &mut alter.operations {
            match op {
                AlterTableOperation::AddColumn { .. }
                | AlterTableOperation::DropColumn { .. }
                | AlterTableOperation::RenameColumn { .. }
                | AlterTableOperation::AlterColumn { .. } => {}
                AlterTableOperation::RenameTable {
                    table_name: RenameTableNameKind::As(new) | RenameTableNameKind::To(new),
                } => {
                    let new_table = renamed_within(&target, new)?;
                    let renamed = target.sibling(&new_table);
                    effects.push(match self.pending.get(&target.live) {
                        Some(source) => Effect::FromCopy {
                            live: renamed,
                            source: source.clone(),
                            fallback: ShadowState::Shadow,
                        },
                        None => {
                            let state = self.shadow.state(&target.live);
                            Effect::Set(renamed, state.unwrap_or(ShadowState::Shadow))
                        }
                    });
                    effects.push(Effect::Set(target.live.clone(), ShadowState::Dropped));
                    *new = ObjectName::from(vec![new_table]);
                }
                _ => return Err(not_allowed("ALTER TABLE with this operation")),
            }
        }
        Ok(effects)
    }

    /// Resolve `name` as a write target, point it at the preview, and note
    /// the write and the schema the host must ensure.
    pub(super) fn target(&mut self, name: &mut ObjectName, verb: &str) -> Result<Target, Refused> {
        let target = names::target(name, verb, self.ns, self.opts)?;
        *name = target.preview_name();
        self.ensure_schema(target.live.0.clone(), target.preview_schema.clone());
        push_unique(&mut self.out.writes, target.live.clone());
        Ok(target)
    }

    /// Queue a copy of the live table when the preview has nothing for it.
    /// From here on the table reads as copied, so the statement's own reads
    /// of it see the copy (the prelude runs before the statement).
    fn copy_on_write(&mut self, target: &Target) {
        if self.shadow.state(&target.live).is_some() {
            return;
        }
        let copy = Prelude::CopyOnWrite {
            live: target.live.clone(),
            preview: (target.preview_schema.clone(), target.live.1.clone()),
        };
        if !self.out.preludes.contains(&copy) {
            self.out.preludes.push(copy);
        }
        self.shadow
            .0
            .insert(target.live.clone(), ShadowState::Shadow);
        self.pending
            .insert(target.live.clone(), target.live.clone());
    }

    pub(super) fn ensure_schema(&mut self, live: String, preview: String) {
        let ensure = Prelude::EnsureSchema { live, preview };
        if !self.out.preludes.contains(&ensure) {
            self.out.preludes.push(ensure);
        }
    }

    pub(super) fn record(&mut self, effect: Effect) {
        match effect {
            Effect::Set(live, state) => {
                self.pending.remove(&live);
                self.shadow.0.insert(live.clone(), state);
                self.out.shadow_updates.push((live, state));
            }
            Effect::FromCopy {
                live,
                source,
                fallback,
            } => {
                let at = self.out.shadow_updates.len();
                self.out.from_copy.push((at, source.clone()));
                self.out.shadow_updates.push((live.clone(), fallback));
                self.shadow.0.insert(live.clone(), fallback);
                self.pending.insert(live, source);
            }
        }
    }
}

/// The one table an UPDATE or DELETE modifies: a plain name, no joins.
fn single_table<'t>(
    table: &'t mut TableWithJoins,
    verb: &str,
) -> Result<&'t mut ObjectName, Refused> {
    match &mut table.relation {
        TableFactor::Table {
            name, args: None, ..
        } if table.joins.is_empty() => Ok(name),
        _ => Err(not_allowed(&format!(
            "{verb} of a join or a non-table target"
        ))),
    }
}

/// `RENAME TO u` or `RENAME TO S.u` stays in the table's schema, and so in
/// the preview; any other schema is refused.
fn renamed_within(target: &Target, new: &ObjectName) -> Result<Ident, Refused> {
    match plain_parts(new, "ALTER TABLE … RENAME TO")?.as_slice() {
        [table] => Ok((*table).clone()),
        [schema, table] if schema.value.eq_ignore_ascii_case(&target.live.0) => {
            Ok((*table).clone())
        }
        _ => Err(Refused(format!(
            "ALTER TABLE … RENAME TO {new}: a rename must stay in the table's schema, {}",
            target.live.0
        ))),
    }
}

pub(super) fn push_unique(list: &mut Vec<(String, String)>, live: (String, String)) {
    if !list.contains(&live) {
        list.push(live);
    }
}

pub(super) fn not_allowed(what: &str) -> Refused {
    Refused(format!("{what} is not allowed in a preview"))
}
