//! Where an Airway sample lands (phase 2b S11): the preview's own schemas for
//! the pipeline's dataset, `preview_<key>__<dataset>` and its `_raw` sibling
//! (airway's buffer schema for `replacing` tables), on a Writer confined to
//! exactly those two.
//!
//! In the registry's host order, as a step's writes are (`airhouse_writes`):
//!
//! 1. the confined Writer is minted first — an Airhouse that cannot confine one
//!    refuses the sample here, before any schema or registry row exists;
//! 2. each schema is ensured through its registry row
//!    (`registry::ensure_schema`: row, then `CREATE SCHEMA` on a system
//!    Writer), so the TTL drop knows it before airway writes a table;
//! 3. the DSN goes back with the dataset overridden to the preview schema.
//!
//! The worker's credential provider re-resolves with the dataset it was handed
//! — already the preview schema — on every reconnect; the registry maps that
//! back to the live dataset, which is re-minted and not ensured again.

use agentic_pipeline::platform::ResolvedPipelineDestination;
use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use super::airhouse_writes::PreviewAirhouse;
use super::workspace::AIRHOUSE_TOO_OLD;
use crate::agentic_wiring::preview_airhouse::PipelineWriter;
use crate::server::previews::registry::{self, RegistryError};

impl PreviewAirhouse {
    /// An Airway sample's destination for `dataset` (module doc). `Err` says
    /// why the sample may not land; nothing was written.
    pub(super) async fn pipeline_destination(
        &self,
        dataset: &str,
    ) -> Result<ResolvedPipelineDestination, String> {
        let asked = dataset.to_ascii_lowercase();
        let (live, again) = match self.registered_live(&asked).await? {
            Some(live) => (live, true),
            None => (asked, false),
        };
        let lives = [live.clone(), format!("{live}_raw")];
        let schemas = lives
            .iter()
            .map(|l| self.ns.schema_for(l).map_err(|r| r.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let dsn = match self
            .ports
            .pipeline_writer(self.workspace_id, &self.ns, &schemas)
            .await?
        {
            PipelineWriter::Dsn(dsn) => dsn,
            PipelineWriter::Unavailable(why) => {
                return Err(format!(
                    "{AIRHOUSE_TOO_OLD} ({why}); the Airway sample is refused and nothing was \
                     written"
                ));
            }
        };
        if !again {
            for live in &lives {
                self.ensure_sample_schema(live).await?;
            }
        }
        Ok(ResolvedPipelineDestination {
            kind: "airhouse".to_string(),
            connection_string: dsn,
            dataset_name_override: Some(schemas[0].clone()),
        })
    }

    /// The live schema `schema` stands in for, when the registry holds it as
    /// one of this preview's (the worker re-resolving the dataset it was
    /// handed): the registry says what is mapped, not the name's shape.
    async fn registered_live(&self, schema: &str) -> Result<Option<String>, String> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT live_schema FROM workspace_preview_schemas \
                 WHERE workspace_id = $1 AND preview_key = $2 AND schema_name = $3 \
                   AND dropped_at IS NULL AND refused_at IS NULL",
                [
                    self.workspace_id.into(),
                    self.ns.key().into(),
                    schema.into(),
                ],
            ))
            .await
            .map_err(|e| format!("reading the preview's schema registry: {e}"))?;
        row.map(|r| r.try_get::<String>("", "live_schema"))
            .transpose()
            .map_err(|e| format!("reading the preview's schema registry: {e}"))
    }

    async fn ensure_sample_schema(&self, live: &str) -> Result<(), String> {
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
        .map(|_| ())
        .map_err(|e| match e {
            RegistryError::Refused(r) => format!(
                "refused in workspace preview {}: {r}. Nothing was written.",
                self.ns.key()
            ),
            other => format!("preparing the preview schema for {live}: {other}"),
        })
    }
}
