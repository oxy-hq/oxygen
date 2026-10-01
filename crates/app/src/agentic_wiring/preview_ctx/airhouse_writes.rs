//! Airhouse writes in a preview run land in the preview's own schemas
//! (phase 2b S8), and every Airhouse read of the run sees them (D6).
//!
//! One `execute_sql` step on the workspace's managed Airhouse, reviewed by
//! [`PreviewAirhouse::review`], in the registry's host order
//! (`previews::registry` module doc):
//!
//! 1. **Can writes land here at all?** An Airhouse that cannot confine a
//!    Writer to named schemas (older than 0.1.49), or none at all, leaves the
//!    step held exactly as phase 2a did ([`Step::Held`]). Never a live write.
//! 2. The shadow map is loaded once, and the step rewritten with it
//!    (`preview_sql::rewrite`). A statement the preview may not run fails the
//!    step; nothing is sent.
//! 3. A Writer confined to the step's preview schemas is minted, before any
//!    schema exists (a refused mint holds the step, as in 1).
//! 4. Each schema is ensured through its registry row
//!    (`registry::ensure_schema`) — never by sending `CREATE SCHEMA`. A schema
//!    the registry refuses fails the step.
//! 5. Copy-on-write: the live table is probed and counted through the run's
//!    connector, and a whole (or, over `OXY_PREVIEW_COW_MAX_ROWS`, empty) copy
//!    planned.
//! 6. **Recorded before sent** (`registry::record_step`): the shadow-map rows
//!    are written, and the run's shared map moved on, before the step executor
//!    sends a statement. A crash after this leaves every relation the step
//!    could have made recorded, so the TTL drop can take it.
//! 7. The copies and the rewritten SQL go back as one batch
//!    (`SqlReview::Rewrite`), which the step executor sends through the run's
//!    connector — verified again there, on the confined Writer.
//!
//! The same connector ([`PreviewAirhouse::connector`]) serves agents and
//! semantic reads: their reads get the overlay from the shared map, and a
//! write of theirs is refused unless it names a relation the map already
//! records (they create nothing the TTL drop would not know to drop).
//!
//! **Review is serialised; send is not.** Reviews of one run take a lock, but
//! a step is sent after its review returns, by the step executor. Top-level
//! steps and a loop with `concurrency: 1` send before the next review, so
//! nothing interleaves. A `loop_sequential` with `concurrency > 1` whose body
//! writes Airhouse can review iteration B while iteration A's copy is
//! recorded but not yet made. A table an unfinished batch carries is not
//! "missing" (`planned`, settled as each batch ends), so B either finds A's
//! copy made or fails on the missing table; and a copy planned again is
//! strict (`CREATE TABLE`), so it can fail but never replace a table. A failed
//! step, never a silently re-copied table and never a live write. Holding a
//! lock per preview key from review through send would need the executor to
//! say when a send ends; it does not.

use std::collections::HashSet;
use std::sync::{Arc, PoisonError, RwLock};

use agentic_automation::SqlReview;
use agentic_connector::DatabaseConnector;
use airhouse::preview_sql::{
    CopyPlan, Prelude, PreviewNamespace, Rewrite, RewriteOptions, ShadowMap, ShadowState, rewrite,
};
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use uuid::Uuid;

use super::PreviewError;
use super::airhouse_batches::{Batches, Planned};
use super::airhouse_copies::{copy_plans, missing_copies};

type Live = (String, String);
use crate::agentic_wiring::preview_airhouse::{PreviewAirhousePorts, Writers, connector_on};
use crate::server::previews::ddl::SchemaCreator;
use crate::server::previews::registry::{self, RegistryError};

/// What the review of an Airhouse step decided.
#[derive(Debug)]
pub(super) enum Step {
    Review(SqlReview),
    /// This Airhouse cannot take preview writes; hold the step, with why.
    Held(String),
}

/// A preview run's Airhouse: its namespace, the shadow map its connector
/// reads, and where its schemas and batches go.
pub(crate) struct PreviewAirhouse {
    pub(super) db: DatabaseConnection,
    pub(super) workspace_id: Uuid,
    pub(super) run_id: String,
    pub(super) ns: PreviewNamespace,
    pub(super) ports: Arc<dyn PreviewAirhousePorts>,
    pub(super) creator: Arc<dyn SchemaCreator>,
    shadow: Arc<RwLock<ShadowMap>>,
    opts: RewriteOptions,
    /// One step reviewed at a time: each loads the map and records its own.
    steps: tokio::sync::Mutex<()>,
    /// Tables a reviewed batch writes, until that batch is done
    /// ([`super::airhouse_batches`]). Their copy, if missing, may be in
    /// flight, so it is not planned again while they are here
    /// ([`Self::rewritten`]); once the batch is done (or on a platform
    /// rebuilt after a crash) a missing copy is planned again.
    planned: Arc<Planned>,
}

impl PreviewAirhouse {
    /// The Airhouse side of run `row`, with its shadow map as it stands.
    pub(crate) async fn open(
        db: &DatabaseConnection,
        row: &entity::workspace_preview_runs::Model,
        ports: Arc<dyn PreviewAirhousePorts>,
    ) -> Result<Self, PreviewError> {
        let ns = crate::server::previews::namespace::for_preview(row)
            .map_err(|e| PreviewError::Unusable(format!("preview key: {e}")))?;
        let shadow = registry::load_shadow(db, row.workspace_id, ns.key())
            .await
            .map_err(|e| PreviewError::Unavailable(format!("reading the shadow map: {e}")))?;
        let opts = RewriteOptions {
            catalog: ports.catalog(),
            read_live_only: read_live_only(&row.options),
        };
        Ok(Self {
            db: db.clone(),
            workspace_id: row.workspace_id,
            run_id: row.run_id.clone(),
            creator: ports.schema_creator(row.workspace_id, &ns),
            ns,
            ports,
            shadow: Arc::new(RwLock::new(shadow)),
            opts,
            steps: tokio::sync::Mutex::new(()),
            planned: Arc::new(Planned::default()),
        })
    }

    /// The connector every Airhouse statement of this run goes through: the
    /// preview Airhouse connector, noting when a reviewed batch is done.
    pub(crate) fn connector(&self) -> Arc<dyn DatabaseConnector> {
        let inner = connector_on(
            self.ports.as_ref(),
            self.workspace_id,
            self.ns.clone(),
            Arc::clone(&self.shadow),
            self.opts.clone(),
        );
        Arc::new(Batches {
            inner,
            planned: Arc::clone(&self.planned),
        })
    }

    /// Review one step's SQL (module doc). `classified_write`: the step
    /// classifier saw a write, so an Airhouse that cannot take one holds the
    /// step before anything else is asked.
    pub(super) async fn review(&self, sql: &str, classified_write: bool) -> Result<Step, String> {
        let _one_at_a_time = self.steps.lock().await;
        if classified_write && let Some(why) = self.cannot_write().await? {
            return Ok(Step::Held(why));
        }
        let (before, rewrite, replanned) = self.rewritten(sql).await?;
        if !writes_anything(&rewrite) {
            return Ok(Step::Review(read_review(&rewrite, &self.ns)));
        }
        if !classified_write && let Some(why) = self.cannot_write().await? {
            return Ok(Step::Held(why));
        }
        let schemas = preview_schemas(&rewrite, &self.ns);
        let confined = self
            .ports
            .confine_writer(self.workspace_id, &self.ns, &schemas)
            .await?;
        if let Writers::Unavailable(why) = confined {
            return Ok(Step::Held(why));
        }
        self.ensure_schemas(&rewrite).await?;
        let conn = self.connector();
        let catalog = self.opts.catalog.as_deref();
        let copies = copy_plans(conn.as_ref(), &rewrite, catalog, &replanned).await?;
        let after = registry::record_step(
            &self.db,
            self.workspace_id,
            self.ns.key(),
            &self.run_id,
            &before,
            &rewrite,
            &copies,
        )
        .await
        .map_err(|e| format!("recording the step in the preview's shadow map: {e}"))?;
        self.set_shadow(after);
        let batch = batch_sql(&copies, &rewrite);
        self.planned.add(batch.clone(), rewrite.writes.clone());
        Ok(Step::Review(SqlReview::Rewrite {
            sql: batch,
            notes: notes(&rewrite, &copies, &schemas, &self.ns),
        }))
    }

    /// The shadow map as it stands (loaded once, and shared with the
    /// connector) and the step rewritten against it. A table the map holds
    /// whose preview copy is missing, and that no unfinished batch carries (a
    /// crash between recording a step and sending it, or a batch that failed
    /// before its copy ran), is taken out of the map and the step rewritten,
    /// so its copy is planned again ([`missing_copies`]) — strictly: returned
    /// in the third place, so the copy never replaces a table a lagging
    /// listing did not show.
    async fn rewritten(&self, sql: &str) -> Result<(ShadowMap, Rewrite, HashSet<Live>), String> {
        let mut before = registry::load_shadow(&self.db, self.workspace_id, self.ns.key())
            .await
            .map_err(|e| format!("reading the preview's shadow map: {e}"))?;
        self.set_shadow(before.clone());
        let step = rewrite(sql, &self.ns, &before, &self.opts).map_err(|r| self.refused(&r))?;
        if !writes_anything(&step) {
            return Ok((before, step, HashSet::new()));
        }
        let conn = self.connector();
        let mut missing = missing_copies(conn.as_ref(), &self.ns, &before, &step).await?;
        missing.retain(|live| !self.planned.contains(live));
        if missing.is_empty() {
            return Ok((before, step, HashSet::new()));
        }
        tracing::warn!(target: "preview", run_id = %self.run_id, ?missing,
            "preview tables recorded but missing (an earlier step never ran); copying again");
        for live in &missing {
            before.0.remove(live);
        }
        self.set_shadow(before.clone());
        let step = rewrite(sql, &self.ns, &before, &self.opts).map_err(|r| self.refused(&r))?;
        Ok((before, step, missing.into_iter().collect()))
    }

    /// Why this deployment's Airhouse cannot take a preview write, if it
    /// cannot.
    async fn cannot_write(&self) -> Result<Option<String>, String> {
        Ok(match self.ports.deployment_writers().await? {
            Writers::Confined => None,
            Writers::Unavailable(why) => Some(why),
        })
    }

    /// Every schema the step writes, through its registry row first.
    async fn ensure_schemas(&self, rewrite: &Rewrite) -> Result<(), String> {
        for prelude in &rewrite.preludes {
            let Prelude::EnsureSchema { live, .. } = prelude else {
                continue;
            };
            registry::ensure_schema(
                &self.db,
                self.creator.as_ref(),
                self.workspace_id,
                &self.ns,
                live,
                &self.run_id,
                registry::schema_ttl(),
            )
            .await
            .map_err(|e| match e {
                RegistryError::Refused(r) => self.refused(&r),
                other => format!("preparing the preview schema for {live}: {other}"),
            })?;
        }
        Ok(())
    }

    fn set_shadow(&self, map: ShadowMap) {
        *self.shadow.write().unwrap_or_else(PoisonError::into_inner) = map;
    }

    fn refused(&self, why: &dyn std::fmt::Display) -> String {
        format!(
            "refused in workspace preview {}: {why}. Nothing was sent.",
            self.ns.key()
        )
    }
}

/// The run's `options.read_live_only`: read live tables even where the
/// preview has a copy (writes are redirected regardless).
fn read_live_only(options: &Value) -> bool {
    options
        .get("read_live_only")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn writes_anything(rewrite: &Rewrite) -> bool {
    !rewrite.preludes.is_empty() || !rewrite.writes.is_empty() || !rewrite.shadow_updates.is_empty()
}

/// A step that only reads: sent as rewritten when a read was pointed at the
/// preview's copy (and the redirect listed), as written otherwise.
fn read_review(rewrite: &Rewrite, ns: &PreviewNamespace) -> SqlReview {
    if rewrite.redirected_reads.is_empty() {
        return SqlReview::Proceed;
    }
    SqlReview::Rewrite {
        sql: rewrite.sql.clone(),
        notes: notes(rewrite, &[], &[], ns),
    }
}

/// The preview schemas a step writes: every schema it ensures, and the home
/// of every table it writes or re-states. Sorted, each once.
fn preview_schemas(rewrite: &Rewrite, ns: &PreviewNamespace) -> Vec<String> {
    let ensured = rewrite.preludes.iter().filter_map(|p| match p {
        Prelude::EnsureSchema { preview, .. } => Some(preview.clone()),
        Prelude::CopyOnWrite { .. } => None,
    });
    let written = rewrite
        .writes
        .iter()
        .chain(rewrite.shadow_updates.iter().map(|(live, _)| live))
        .filter_map(|(schema, _)| ns.schema_for(schema).ok());
    let mut schemas: Vec<String> = ensured.chain(written).collect();
    schemas.sort();
    schemas.dedup();
    schemas
}

/// A step that only ensured a schema has nothing left to send; the executor
/// still sends something, so it gets a read of nothing.
const NOTHING_TO_SEND: &str = "SELECT 1 AS preview_schema_ensured LIMIT 0";

/// The copies, then the step: one batch on one connection.
fn batch_sql(copies: &[CopyPlan], rewrite: &Rewrite) -> String {
    let statements: Vec<&str> = copies
        .iter()
        .map(|c| c.statement.as_str())
        .chain((!rewrite.sql.trim().is_empty()).then_some(rewrite.sql.as_str()))
        .collect();
    if statements.is_empty() {
        NOTHING_TO_SEND.to_string()
    } else {
        statements.join(";\n")
    }
}

/// The step result's `preview` note: every redirect, listed (D6).
fn notes(
    rewrite: &Rewrite,
    copies: &[CopyPlan],
    schemas: &[String],
    ns: &PreviewNamespace,
) -> Value {
    let pair = |(schema, table): &(String, String)| {
        json!({
            "live": format!("{schema}.{table}"),
            "preview": ns.schema_for(schema).map(|s| format!("{s}.{table}")).unwrap_or_default(),
        })
    };
    let copies: Vec<Value> = copies
        .iter()
        .map(|c| {
            json!({
                "live": format!("{}.{}", c.live.0, c.live.1),
                "state": if c.state == ShadowState::Partial { "partial" } else { "shadow" },
            })
        })
        .collect();
    json!({
        "rewritten": true,
        "writes": rewrite.writes.iter().map(pair).collect::<Vec<_>>(),
        "reads": rewrite.redirected_reads.iter().map(pair).collect::<Vec<_>>(),
        "copies": copies,
        "schemas": schemas,
    })
}

#[cfg(test)]
#[path = "airhouse_writes_tests.rs"]
mod tests;
