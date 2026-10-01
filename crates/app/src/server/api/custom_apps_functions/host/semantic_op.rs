//! `ctx.semantic.query` — compile a semantic query against the compiled model
//! this invocation reads, then answer it from a rollup or the warehouse.
//!
//! The trait method (`host.rs`) re-scopes the run's staging pin around this:
//! a host call runs on a task of its own, and the rollup short-circuit
//! (`preagg_context`) reads the pin from the task to stand aside for a
//! branch's model.

use std::path::PathBuf;

use agentic_semantic::compile::PreaggContext;

use super::*;

impl ProjectFunctionHost {
    pub(super) async fn semantic_query_unpinned(
        &self,
        mut spec: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let scoped = take_reach_scope(&mut spec)?;
        let mut query: SemanticQueryConfig = serde_json::from_value(spec)
            .map_err(|e| format!("invalid semantic query spec: {e}"))?;

        let cm = &self.proj_ctx.workspace_manager().config_manager;
        // The compiled revision this invocation reads, materialised — the same
        // `scan_dir` the `/semantic-query` route uses. This read
        // `semantics_scan_path()`, the raw working copy: an IDE node answered
        // from whatever was on disk, and a diskless replica from a directory
        // that is not there. `_scan` keeps a materialised tempdir alive until
        // the compile below is done.
        let _scan = crate::server::api::semantic_scan::scan_dir(cm)
            .await
            .map_err(|e| {
                format!("ctx.semantic.query: no compiled semantic model available: {e}")
            })?;
        let scan_path = _scan.path().to_path_buf();
        let pre_loaded_layer = if scoped {
            Some(self.reach_scoped_layer(&scan_path, &mut query).await?)
        } else {
            None
        };
        let databases: Vec<oxy_airlayer_compat::DatabaseConfig> = cm
            .list_databases()
            .iter()
            .map(|db| oxy_airlayer_compat::database_config(db.name.clone(), db.dialect()))
            .collect();
        let preagg = self.rollup_context();

        // Sentry hubs are per thread; keep the invocation's on the blocking pool.
        let hub = sentry::Hub::current();
        let compiled = tokio::task::spawn_blocking(move || {
            sentry::Hub::run(hub, || {
                resolve_and_compile(
                    &scan_path,
                    &databases,
                    &query,
                    preagg.as_ref(),
                    pre_loaded_layer,
                )
            })
        })
        .await
        .map_err(|e| format!("semantic compile task panicked: {e}"))?
        .map_err(|e| format!("semantic compile failed: {e}"))?;
        self.answer_compiled(compiled).await
    }

    /// A scoped query needs the layer before it compiles, to know which of its
    /// views are bound. Loaded once and handed to the compile, so the
    /// directory is walked one time, not two.
    async fn reach_scoped_layer(
        &self,
        scan_path: &std::path::Path,
        query: &mut SemanticQueryConfig,
    ) -> Result<oxy_airlayer_compat::SemanticLayer, String> {
        let scan_for_layer: PathBuf = scan_path.to_path_buf();
        // Sentry hubs are per thread; keep the invocation's on the blocking pool.
        let hub = sentry::Hub::current();
        let layer = tokio::task::spawn_blocking(move || {
            sentry::Hub::run(hub, || {
                oxy_airlayer_compat::load_layer_from_dir(&scan_for_layer)
            })
        })
        .await
        .map_err(|e| format!("semantic layer task panicked: {e}"))?
        .map_err(|e| format!("semantic layer failed to load: {e}"))?;
        crate::server::api::operating_graph::binding::apply_reach_scope(
            &self.db,
            self.org_id,
            &layer,
            &self.reach,
            query,
        )
        .await
        .map_err(|e| format!("ctx.semantic.query: {e}"))?;
        Ok(layer)
    }

    /// The rollup short-circuit, resolved exactly as
    /// `/api/projects/{id}/semantic-query` resolves it — a bundle asking the
    /// same question through `ctx.semantic` instead of the HTTP route must not
    /// silently drop to the warehouse. `None` when this composition carries no
    /// Layer-1 cache (the scheduled path), and under a staging pin, whose
    /// branch model no rollup was built from.
    ///
    /// The threshold comes from THIS workspace's own
    /// `pre_aggregations.refresh_worker.renewal_threshold` when the process
    /// publishes no global value — the same key the rebuild cycle reads.
    fn rollup_context(&self) -> Option<PreaggContext> {
        let cm = &self.proj_ctx.workspace_manager().config_manager;
        let workspace_id = self.proj_ctx.workspace_manager().workspace_id;
        let renewal_threshold_secs = self.preagg.renewal_threshold_secs_or(cm);
        crate::server::preagg_context::preagg_context(
            workspace_id,
            self.preagg.cache.clone(),
            Some(renewal_threshold_secs),
            // A read surface: `ctx.semantic` renders a number for a bundle to
            // display, and the badge says which tier answered.
            crate::server::preagg_context::RollupFreshness::ServeStale,
        )
    }

    /// Read a rollup when one answered, else the warehouse.
    async fn answer_compiled(&self, compiled: CompiledQuery) -> Result<serde_json::Value, String> {
        let (sql, database_name) = match compiled {
            CompiledQuery::Warehouse { sql, database_name } => (sql, database_name),
            CompiledQuery::Preaggregation {
                preagg_sql,
                source,
                warehouse_sql,
                warehouse_database,
            } => {
                // A rollup that won't read is not a failed query — the same
                // question has a warehouse answer, and the variant carries the
                // SQL for it. Same posture as the `/semantic-query` route.
                match read_rollup(&preagg_sql, &source, FUNCTION_MAX_ROWS).await {
                    Ok(result) => {
                        enforce_result_byte_cap(&result)?;
                        return Ok(result);
                    }
                    Err(e) => {
                        tracing::warn!(
                            remote = source.is_remote(),
                            error = %e,
                            "ctx.semantic: preagg rollup read failed; answering from the warehouse instead"
                        );
                        (warehouse_sql, warehouse_database)
                    }
                }
            }
        };
        let connector = self.connect(&database_name).await?;
        let (rows, truncated) =
            query_with_truncation(&self.query_exec, connector, &sql, FUNCTION_MAX_ROWS).await?;
        let result = serde_json::json!({ "rows": rows, "truncated": truncated });
        enforce_result_byte_cap(&result)?;
        Ok(result)
    }
}

/// `scope: "reach"` is oxy's, not the query's: peeled off before the config
/// parses, then honoured by pinning the bound view's key to the keys the
/// caller's places carry. Anything else in `scope` is a typo, and a typo that
/// silently answered everything is the one outcome this option exists to
/// prevent.
fn take_reach_scope(spec: &mut serde_json::Value) -> Result<bool, String> {
    let scope = spec
        .as_object_mut()
        .and_then(|m| m.remove("scope"))
        .map(|v| v.as_str().map(str::to_string).unwrap_or_default());
    match scope.as_deref() {
        None | Some("") => Ok(false),
        Some("reach") => Ok(true),
        Some(other) => Err(format!(
            "invalid semantic query spec: unknown scope `{other}`"
        )),
    }
}
