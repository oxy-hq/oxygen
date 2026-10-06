//! [`ProjectContext`] implementation for [`OxyProjectContext`].

use std::sync::Arc;

use agentic_analytics::config::ResolvedModelInfo;
use agentic_connector::{ConnectorConfig, DatabaseConnector};
use agentic_pipeline::SharedMetricSink;
use agentic_pipeline::platform::{ProjectContext, ResolvedPipelineDestination};
use async_trait::async_trait;
use entity::workspace_members::WorkspaceRole;
use oxy::config::model::DatabaseType;

use crate::server::preagg_context::RollupFreshness;

use super::{
    OxyProjectContext, airhouse_wire_params, pg_wire_dsn, resolve_connector_impl,
    resolve_model_impl, resolve_pre_built_airhouse,
};

#[async_trait]
impl ProjectContext for OxyProjectContext {
    fn workspace_id(&self) -> uuid::Uuid {
        self.workspace_manager.workspace_id
    }

    /// Project timezone from `config.yml`, parsed to a [`chrono_tz::Tz`].
    ///
    /// Deliberately lenient: an unrecognized IANA name warns and falls back to
    /// UTC rather than failing. `config.yml` is project-wide, so a typo here
    /// should not take down every agent and the server. (The per-agent
    /// `.agentic.yml` `timezone:` is validated strictly instead — a bad value
    /// fails just that agent at build time via `ConfigError::InvalidTimezone`.)
    fn timezone(&self) -> Option<chrono_tz::Tz> {
        let raw = self.workspace_manager.config_manager.timezone()?;
        match raw.parse::<chrono_tz::Tz>() {
            Ok(tz) => Some(tz),
            Err(_) => {
                tracing::warn!(
                    timezone = raw,
                    "config.yml `timezone:` is not a valid IANA timezone name \
                     (expected e.g. `America/Los_Angeles`); falling back to UTC. \
                     Relative dates like \"yesterday\" may resolve a day off until \
                     the value is corrected in config.yml."
                );
                None
            }
        }
    }

    /// `None` while holding writes: the pipeline would build a bare config
    /// unwrapped. [`Self::resolve_pre_built_connector`] answers instead.
    async fn resolve_connector(&self, db_name: &str) -> Option<ConnectorConfig> {
        if self.holds_writes() {
            return None;
        }
        resolve_connector_impl(db_name, &self.workspace_manager).await
    }

    /// Airhouse only — except while holding writes, when every database is
    /// pre-built here, held.
    async fn resolve_pre_built_connector(
        &self,
        db_name: &str,
    ) -> Option<Arc<dyn DatabaseConnector>> {
        if self.holds_writes() {
            return self.held_pre_built(db_name).await;
        }
        resolve_pre_built_airhouse(
            db_name,
            &self.workspace_manager,
            self.subject,
            self.role.clone(),
        )
        .await
    }

    /// `None` while holding writes: a destination is where a pipeline writes.
    async fn resolve_pipeline_destination(
        &self,
        db_name: &str,
        dataset_name: &str,
    ) -> Option<ResolvedPipelineDestination> {
        if self.holds_writes() {
            return None;
        }
        let db = self
            .workspace_manager
            .config_manager
            .resolve_database(db_name)
            .ok()?;
        match &db.database_type {
            // Postgres/Redshift: `resolve_connector` already substituted
            // secrets into the connection params.
            // Per-org OLTP. The pipeline writes into `raw_<dataset_name>` as
            // that source's own writer role — NOT the analyst the query path
            // resolves, which is read-only by design.
            //
            // `dataset_name` picks the writer, so two pipelines landing into one
            // org's database each get their own credential and neither can
            // touch the other's schema. That is why this resolver takes it:
            // without it there is only a database name, and a database holds
            // every source.
            DatabaseType::PostgresManaged(_) => {
                let workspace_id = self.workspace_manager.workspace_id;
                let writer = oxy_oltp::schema::WriterRef::pipeline(dataset_name)
                    .map_err(|e| {
                        tracing::warn!(
                            dataset_name,
                            "airway: not a usable postgres_managed source name: {e}"
                        );
                    })
                    .ok()?;
                let db = oxy::database::client::establish_connection().await.ok()?;
                let conn =
                    oxy_oltp::resolver::resolve_writer_connection(&db, workspace_id, &writer)
                        .await
                        .map_err(|e| {
                            tracing::warn!(
                                db_name,
                                dataset_name,
                                "airway: no OLTP writer for this source — provision it with \
                         `oxy oltp provision --org <org> --writer pipeline:{dataset_name}`: {e}"
                            );
                        })
                        .ok()?;
                Some(ResolvedPipelineDestination {
                    kind: "postgres".to_string(),
                    connection_string: conn.dsn,
                    // `raw_<source>`, the only schema this credential can write.
                    dataset_name_override: Some(conn.schema),
                })
            }
            DatabaseType::Postgres(_) | DatabaseType::Redshift(_) => {
                match self.resolve_connector(db_name).await? {
                    ConnectorConfig::Postgres(c) | ConnectorConfig::Redshift(c) => {
                        Some(ResolvedPipelineDestination {
                            kind: "postgres".to_string(),
                            connection_string: pg_wire_dsn(
                                &c.user,
                                &c.password,
                                &c.host,
                                c.port,
                                &c.database,
                            ),
                            dataset_name_override: None,
                        })
                    }
                    _ => None,
                }
            }
            // Airhouse speaks the Postgres wire protocol; `airhouse_managed`
            // mints an ephemeral per-subject credential here.
            DatabaseType::Airhouse(_) | DatabaseType::AirhouseManaged(_) => {
                match airhouse_wire_params(
                    &db,
                    &self.workspace_manager,
                    self.subject,
                    self.role.clone(),
                )
                .await
                {
                    Ok((host, port, user, password, database)) => {
                        Some(ResolvedPipelineDestination {
                            kind: "airhouse".to_string(),
                            connection_string: pg_wire_dsn(
                                &user, &password, &host, port, &database,
                            ),
                            dataset_name_override: None,
                        })
                    }
                    Err(e) => {
                        tracing::warn!(
                            db = %db_name,
                            "airway airhouse destination resolve failed: {e}"
                        );
                        None
                    }
                }
            }
            _ => None,
        }
    }

    async fn resolve_model(
        &self,
        model_ref: Option<&str>,
        has_explicit_model: bool,
    ) -> Option<ResolvedModelInfo> {
        resolve_model_impl(model_ref, has_explicit_model, &self.workspace_manager).await
    }

    async fn resolve_agent_yaml(&self, agent_id: &str) -> Option<String> {
        // Serve the agent YAML from `agent_definitions`; the reader returns
        // None (→ filesystem read) on any miss.
        //
        // `agent_id` may be a path-like reference (`agents/foo.agentic.yml`)
        // or a bare stem (`foo`); `agent_definitions` is keyed by the
        // strict-typed `AgenticAgent.name` (stem), so we normalise.
        let name = agent_id
            .trim_end_matches(".agentic.yml")
            .trim_end_matches(".agentic.yaml")
            .rsplit('/')
            .next()
            .unwrap_or(agent_id);
        self.compiled_agent(name)
            .await
            .unwrap_or(None)
            .and_then(|definition| serde_yaml::to_string(&definition).ok())
    }

    /// From the revision this context is pinned to, or the working copy on
    /// a node reading one — `ConfigManager` decides, on every role. See
    /// `context_documents.rs`.
    async fn resolve_context_documents(
        &self,
        patterns: &[String],
    ) -> Result<Option<Vec<String>>, agentic_automation::WorkspaceReadError> {
        self.context_documents(patterns).await
    }

    async fn resolve_secret(&self, var_name: &str) -> Option<String> {
        match self
            .workspace_manager
            .secrets_manager
            .resolve_secret(var_name)
            .await
        {
            Ok(Some(v)) => return Some(v),
            Ok(None) => {}
            Err(e) => tracing::warn!(
                key_var = %var_name,
                error = %e,
                "secrets_manager.resolve_secret failed; falling back to std::env::var"
            ),
        }
        super::env_fallback(&self.workspace_manager.secrets_manager, var_name)
    }

    async fn persist_secret(&self, var_name: &str, value: &str) -> Result<(), String> {
        // Atomic upsert: a single UPDATE when the secret exists (no
        // remove/recreate window), create when it doesn't. On a fresh
        // env-only token this writes a DB secret that then takes
        // precedence (DB-first fallback storage). created_by is the
        // requesting user — `secrets.created_by` is NOT NULL with an FK to
        // `users`, so a hardcoded Uuid::nil() INSERT (a first-time write,
        // e.g. rotating QB_REFRESH_TOKEN into a project that never had it)
        // violates fk_secrets_created_by. self.subject is set by
        // build_project_context(user.id); nil only for subject-less cron.
        if self.holds_writes() {
            return Err(self.hold_wording().refusal(&format!("Secret `{var_name}`")));
        }
        self.workspace_manager
            .secrets_manager
            .upsert_secret(
                var_name,
                value,
                self.subject.unwrap_or_else(uuid::Uuid::nil),
            )
            .await
            .map_err(|e| format!("persist secret `{var_name}`: {e}"))
    }

    fn metric_sink(&self) -> Option<SharedMetricSink> {
        // Only hand back a sink when an observability store is actually
        // registered. A run with `OXY_OBSERVABILITY_BACKEND` unset (or a
        // removed label) leaves `get_global()` as `None` — not `--enterprise`,
        // which does not gate observability — and the adapter's first call
        // would just log-and-skip anyway —
        // returning `None` here keeps the pipeline hot path free of the
        // atomic load + tracing::warn on every query.
        oxy_observability::global::get_global()?;
        Some(Arc::new(
            super::super::metric_sink::OxyAnalyticsMetricSink::new(),
        ))
    }

    fn metric_tree_runner(&self) -> Option<Arc<dyn agentic_analytics::MetricTreeRunner>> {
        // Background paths (scheduler, recovery) leave subject + role unset;
        // the metric-tree ops need both to mint Airhouse credentials, so we
        // only expose the runner for HTTP-driven runs that carried them in.
        let user_id = self.subject?;
        let role = self.role.clone()?;
        Some(super::super::metric_tree_runner::make_runner(
            self.workspace_manager.clone(),
            user_id,
            role,
            self.runner_preagg(RollupFreshness::ServeStale),
            self.semantic_scan_root(),
            self.holds_writes(),
        ))
    }

    fn metric_tree_runner_system(&self) -> Option<Arc<dyn agentic_analytics::MetricTreeRunner>> {
        // Cron-driven scans don't have a user — mint with nil UUID + Owner
        // so the Airhouse credential resolves with admin read access.
        // Real per-user scoping is enforced upstream when a human triggers
        // a scan via the HTTP endpoint.
        //
        // THREAT-MODEL: anomaly rows produced here (observed, expected,
        // period_*) are visible to all workspace members including Viewers.
        // If warehouse-level RLS is part of the deployment threat model,
        // consider restricting the inbox payload for non-admin roles or
        // running the system scan with a read-only service account that
        // matches the most restrictive tenant RLS policy.
        // The rollup short-circuit the request-driven sibling gets, with one
        // difference: `RequireFresh`. The scheduled scan persists what it
        // computes to the Insights Inbox and an unhealthy transition pages
        // Slack, so a rollup whose newest buckets are missing must fall
        // through to the warehouse rather than read as a drop. Without the
        // cache here at all, the scheduled scan and the manual
        // `POST /semantic/anomalies/scan` of the SAME monitor answered from
        // different tiers and could write different anomalies for the same
        // monitor and granularity.
        //
        // Off while holding writes: its callers persist what it computes.
        if self.holds_writes() {
            return None;
        }
        Some(super::super::metric_tree_runner::make_runner(
            self.workspace_manager.clone(),
            uuid::Uuid::nil(),
            WorkspaceRole::Owner,
            self.runner_preagg(RollupFreshness::RequireFresh),
            // The compile-boundary scan root when this context carries one.
            // Without it the runner parses the semantic model out of
            // `working_copy().root()`, and every caller of this accessor is a
            // background one that may be running where that directory is not —
            // or, for workspace health, where it holds edits nobody promoted.
            self.semantic_scan_root(),
            false,
        ))
    }

    /// Off while holding writes: the anomaly tools write anomaly rows.
    fn anomaly_store(&self) -> Option<Arc<dyn agentic_analytics::anomaly_store::AnomalyStore>> {
        if self.holds_writes() {
            return None;
        }
        let db = self.db.clone()?;
        Some(Arc::new(oxy_metric_monitoring::store::OxyAnomalyStore {
            db,
        }))
    }

    /// Off while holding writes: a scan persists anomalies.
    fn as_monitor_scan_port(&self) -> Option<&dyn agentic_pipeline::platform::MonitorScanPort> {
        (!self.holds_writes()).then_some(self as _)
    }

    fn compile_dispatcher(
        &self,
    ) -> Option<std::sync::Arc<dyn agentic_pipeline::platform::CompileDispatcher>> {
        // The dispatcher only needs the DB; the rest comes from the
        // TaskSpec::Compile payload. Background / CLI paths that leave
        // `db` unset get None and Compile fails with a clear message. Off
        // while holding writes: a compile writes a revision.
        if self.holds_writes() {
            return None;
        }
        let db = self.db.clone()?;
        Some(std::sync::Arc::new(
            crate::agentic_wiring::compile_dispatcher::OxyCompileDispatcher::new(db),
        ))
    }

    /// A context built for a held request is a preview: the pipeline builds it
    /// no automation runner, gives it no builder bridges, and refuses its
    /// Airway steps. See [`super::preview_hold`].
    fn is_workspace_preview(&self) -> bool {
        self.holds_writes()
    }

    /// The custom app whose staging ask this context holds for, so its run is
    /// stamped with the app.
    fn staging_app_id(&self) -> Option<uuid::Uuid> {
        self.hold_scope()?.app_id()
    }
}
