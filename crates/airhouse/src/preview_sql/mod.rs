//! Keeping a workspace preview's Airhouse writes in the preview's own schemas.
//!
//! A preview runs a branch's procedures against real data. Its reads may see
//! live tables; its writes must never land in one. Live schema `S` has a
//! stand-in, `preview_<key>__S` ([`PreviewNamespace`]), and three fences keep
//! writes there: [`rewrite`] redirects every write target of a step's SQL into
//! the namespace, [`verify`] independently refuses any statement the preview
//! connector sends that writes outside it, and the Airhouse credential is
//! scoped to the namespace's schemas. This module is the first two.
//!
//! Both work on the tree, not the text, as `sql_rules` does: SQL is parsed as
//! DuckDB, SQL that does not parse is refused, and what is sent is re-rendered
//! from the checked tree, so DuckDB never finds a statement sqlparser did not.
//!
//! Names:
//! - A write target must be `S.t` (or `<catalog>.S.t` in the workspace's own
//!   catalog). It becomes `"preview_<key>__S"."t"`. An unqualified target is
//!   refused: what it resolves to depends on the session.
//! - A read of `S.t` becomes the preview's copy when the [`ShadowMap`] says the
//!   preview has one, and otherwise stays live. An unqualified read `t` that is
//!   not a CTE in scope is looked up as `main.t`.
//! - Live schemas containing `__` or starting with `preview_` have no stand-in,
//!   so the mapping stays one-to-one; reads of this preview's own schemas are
//!   left alone, which makes [`overlay_reads`] idempotent.
//!
//! A write to a table the preview has no copy of first copies it
//! ([`Prelude::CopyOnWrite`]), so an `INSERT` or `DELETE` sees what it would
//! in prod. A full replacement (`CREATE OR REPLACE`, `TRUNCATE`, `DROP`) copies
//! nothing. `CREATE SCHEMA` is not sent: the host ensures the stand-in instead.
//! Engine and session statements (`ATTACH`, `SET`, `COPY`, `CALL`, `PRAGMA`,
//! `INSTALL`, `DROP SCHEMA`, …) and table functions that can write around these
//! rules (`postgres_*`, `ducklake_*`, `query`, …) are refused outright.

mod copy;
mod names;
mod namespace;
mod overlay;
mod rewrite;
mod rewrite_ddl;
mod transactions;
mod verify;

use std::collections::HashMap;

pub use crate::sql_rules::Refused;
pub use copy::{
    COW_MAX_ROWS_VAR, CopyPlan, DEFAULT_COW_MAX_ROWS, Prelude, cow_max_rows, parse_cow_max_rows,
};
pub use namespace::PreviewNamespace;

use crate::sql_parse::on_sql_stack;
use crate::sql_rules::{is_read_only, leading_keyword};

/// What the preview holds for a live table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowState {
    /// A complete copy: built by a transform, written by a step, or copied
    /// whole before a write.
    Shadow,
    /// The live table was over the copy-on-write cap, so the copy started
    /// empty and holds only what the preview wrote.
    Partial,
    /// An Airway sample loaded a bounded window of it.
    Sample,
    /// The preview dropped it; reads resolve to the missing preview table and
    /// fail as they would in prod.
    Dropped,
}

/// Every live `(schema, table)` the preview has something for, keyed
/// lowercase. Reads of these resolve to the preview's copy.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct ShadowMap(pub HashMap<(String, String), ShadowState>);

impl ShadowMap {
    pub fn state(&self, live: &(String, String)) -> Option<ShadowState> {
        self.0.get(live).copied()
    }

    /// Apply updates in order. After a rewritten step, use
    /// [`ShadowMap::apply_rewrite`], which also settles its copies.
    pub fn apply(&mut self, updates: &[((String, String), ShadowState)]) {
        for (live, state) in updates {
            self.0.insert(live.clone(), *state);
        }
    }
}

#[derive(Default, Clone, Debug)]
pub struct RewriteOptions {
    /// The workspace's DuckLake catalog. A three-part name, or a two-part
    /// `<catalog>.t`, is accepted only in it; with `None`, every three-part
    /// name is refused.
    pub catalog: Option<String>,
    /// Read live tables even where the preview has a copy (the run option
    /// `read_live_only`). Writes are redirected regardless.
    pub read_live_only: bool,
}

/// A step's SQL, redirected into the preview.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct Rewrite {
    /// The statements to send, re-rendered from the rewritten tree and joined
    /// with `";\n"`. May be empty (a lone `CREATE SCHEMA`).
    pub sql: String,
    /// Run these first, in order.
    pub preludes: Vec<Prelude>,
    /// Live `(schema, table)`s whose preview copies this SQL writes.
    pub writes: Vec<(String, String)>,
    /// Live `(schema, table)`s read from the preview's copy instead.
    pub redirected_reads: Vec<(String, String)>,
    /// What the statements leave the preview holding, in order. Copies are
    /// not here: they settle when the host passes its [`CopyPlan`]s to
    /// [`ShadowMap::apply_rewrite`], which applies these after them.
    pub shadow_updates: Vec<((String, String), ShadowState)>,
    /// `(i, source)`: `shadow_updates[i]` takes the state of the copy of
    /// `source`, when the host made one.
    from_copy: Vec<(usize, (String, String))>,
}

/// Redirect every write in `sql` into `ns`, and every read the preview has a
/// copy of to that copy. Statement by statement: a later statement sees what
/// an earlier one wrote. Refuses what a preview may not run.
pub fn rewrite(
    sql: &str,
    ns: &PreviewNamespace,
    shadow: &ShadowMap,
    opts: &RewriteOptions,
) -> Result<Rewrite, Refused> {
    on_sql_stack(sql, "a preview", |statements| {
        transactions::check(&statements)?;
        let mut rewriter = rewrite::Rewriter::new(ns, shadow.clone(), opts);
        for statement in statements {
            rewriter.statement(statement)?;
        }
        Ok(rewriter.finish())
    })
}

/// The read half of [`rewrite`], for SQL the preview connector sends on behalf
/// of agents and semantic queries: every statement must be a read, and reads
/// the preview has a copy of go to the copy. Idempotent.
pub fn overlay_reads(
    sql: &str,
    ns: &PreviewNamespace,
    shadow: &ShadowMap,
    opts: &RewriteOptions,
) -> Result<String, Refused> {
    on_sql_stack(sql, "a preview", |statements| {
        let mut sent = Vec::new();
        for mut statement in statements {
            if !is_read_only(&statement) {
                return Err(Refused(format!(
                    "{} is a write; in a preview only a procedure step's SQL may write",
                    leading_keyword(&statement)
                )));
            }
            overlay::apply(ns, shadow, opts, &mut statement)?;
            sent.push(statement.to_string());
        }
        Ok(sent.join(";\n"))
    })
}

/// Refuse `sql` unless every table, view or schema it writes is in `ns` (and,
/// when named with a catalog, in the workspace's `catalog`), and return its
/// statements re-rendered from the checked tree; send those. Independent of
/// [`rewrite`]: it is the fence for SQL that never went through it.
pub fn verify(
    sql: &str,
    ns: &PreviewNamespace,
    catalog: Option<&str>,
) -> Result<Vec<String>, Refused> {
    verify_statements(sql, ns, catalog).map(|v| v.into_iter().map(|s| s.sql).collect())
}

/// What one statement [`verify_statements`] passed does, so the preview
/// connector can pick the credential it runs on and keep a transaction
/// balanced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatementRole {
    /// Writes nothing.
    Read,
    /// Opens the batch's transaction.
    Begin,
    /// `COMMIT` or `ROLLBACK`: closes it.
    End,
    /// Writes these preview schemas: lowercase, sorted, each one `ns` owns.
    Write(Vec<String>),
    /// `CREATE SCHEMA` or `DROP SCHEMA` of one of `ns`'s schemas. The preview
    /// connector refuses these: a preview schema is created only after its
    /// registry row (or the TTL sweep never drops it), and dropped only by
    /// that sweep.
    SchemaDdl(Vec<String>),
}

/// A statement [`verify_statements`] passed, re-rendered from the checked
/// tree: send `sql`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verified {
    pub sql: String,
    pub role: StatementRole,
    /// Every relation the statement writes — creates, fills, alters, renames
    /// to or drops — as `(preview schema, relation)`, lowercase, sorted. Empty
    /// for a read.
    pub relations: Vec<(String, String)>,
}

/// [`verify`], statement by statement, with the preview schemas each one
/// writes. One refused statement refuses the whole batch.
pub fn verify_statements(
    sql: &str,
    ns: &PreviewNamespace,
    catalog: Option<&str>,
) -> Result<Vec<Verified>, Refused> {
    on_sql_stack(sql, "a preview", |statements| {
        verify::verify(&statements, ns, catalog)
    })
}

#[cfg(test)]
mod fence_tests;
#[cfg(test)]
mod tests;
