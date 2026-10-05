//! [`WorkspaceContext`] for [`PreviewPlatformContext`]: reads from the staging
//! revision, writes held. Every method is stated.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use agentic_automation::workspace::IntegrationConfig;
use agentic_automation::{
    ContextRoot, HttpReview, SqlReview, WorkspaceContext, WorkspaceReadError,
};
use agentic_connector::{DatabaseConnector, SqlDialect, StringLiteral};
use async_trait::async_trait;
use oxy::config::model::DatabaseType;

use super::airhouse_writes::{PreviewAirhouse, Step};
use super::{PreviewPlatformContext, airhouse, names};
use crate::agentic_wiring::string_literal::string_literal_of;
use crate::server::previews::hold::HoldingConnector;
use crate::server::previews::sql_kind::{StatementKind, classify, first_non_read, is_all_read};

impl PreviewPlatformContext {
    /// The connector for `name` (scoped to this run or unscoped), built once
    /// per run. Every connector this platform hands out comes from here: the
    /// preview Airhouse connector for the managed Airhouse ([`Self::airhouse`]),
    /// a held one for everything else.
    pub(super) async fn held_connector(
        &self,
        name: &str,
    ) -> Result<Arc<dyn DatabaseConnector>, String> {
        let name = self.own_name(name)?.to_string();
        let cell = {
            let mut cells = self.connectors.lock().await;
            cells.entry(name.clone()).or_default().clone()
        };
        cell.get_or_try_init(|| async {
            if let Some(airhouse) = self.preview_airhouse(&name)? {
                return Ok(airhouse.connector());
            }
            let inner = self.pinned(self.raw_connector(&name)).await?;
            Ok(Arc::new(HoldingConnector::new(inner, name.clone())) as Arc<dyn DatabaseConnector>)
        })
        .await
        .cloned()
    }

    /// The run's Airhouse side, when `name` is the workspace's managed
    /// Airhouse. An `airhouse` database with credentials of its own is not:
    /// the preview's schemas live only in the workspace's tenant, so its
    /// writes stay held.
    pub(super) fn preview_airhouse(&self, name: &str) -> Result<Option<&PreviewAirhouse>, String> {
        Ok(match self.database_type(name)? {
            DatabaseType::AirhouseManaged(_) => self.airhouse.as_ref(),
            _ => None,
        })
    }

    /// The unwrapped connector — never returned from this module unwrapped.
    /// A managed Airhouse gets the preview's own Reader credential.
    async fn raw_connector(&self, name: &str) -> Result<Arc<dyn DatabaseConnector>, String> {
        match self.database_type(name)? {
            DatabaseType::AirhouseManaged(_) => airhouse::reader_connector(self.workspace_id).await,
            _ => self
                .inner
                .build_connector_for(name)
                .await
                .map_err(|e| e.to_string()),
        }
    }

    fn database_type(&self, name: &str) -> Result<DatabaseType, String> {
        self.inner
            .workspace_manager()
            .config_manager
            .resolve_database(name)
            .map(|db| db.database_type.clone())
            .map_err(|_| format!("database `{name}` is not in the previewed branch's config.yml"))
    }

    /// The dialect the classifier reads `name`'s SQL in — the dialect its
    /// connector reports, without building one.
    fn dialect_of(&self, name: &str) -> Result<SqlDialect, String> {
        Ok(match self.database_type(name)? {
            DatabaseType::ClickHouse(_) => SqlDialect::CLICKHOUSE,
            DatabaseType::Bigquery(_) => SqlDialect::BigQuery,
            DatabaseType::Snowflake(_) => SqlDialect::Snowflake,
            DatabaseType::Postgres(_)
            | DatabaseType::Redshift(_)
            | DatabaseType::PostgresManaged(_) => SqlDialect::Postgres,
            // Airhouse is DuckLake behind pgwire; its connector reports DuckDB.
            DatabaseType::DuckDB(_)
            | DatabaseType::MotherDuck(_)
            | DatabaseType::Airhouse(_)
            | DatabaseType::AirhouseManaged(_) => SqlDialect::DuckDb,
            DatabaseType::Mysql(_) => SqlDialect::Other("MySQL"),
            DatabaseType::DOMO(_) => SqlDialect::Other("DOMO"),
        })
    }
}

/// What a held `execute_sql` step records.
fn hold_for(database: &str, kind: Option<&StatementKind>) -> SqlReview {
    let (verb, targets, why) = match kind {
        Some(StatementKind::Write { verb, targets }) => (
            verb.clone(),
            targets.clone(),
            format!("`{database}` is not written"),
        ),
        Some(StatementKind::Unclassified(reason)) => (
            "UNCLASSIFIED".to_string(),
            vec![],
            format!("the SQL could not be classified as a read ({reason})"),
        ),
        Some(StatementKind::Read) | None => ("UNKNOWN".to_string(), vec![], "held".to_string()),
    };
    SqlReview::Hold {
        reason: format!("{why} in a workspace preview; nothing was sent"),
        verb,
        targets,
    }
}

/// The phase 2a hold, for a managed-Airhouse write this deployment cannot
/// land in the preview: `why` says what the Airhouse answered.
fn held_airhouse(database: &str, kind: Option<&StatementKind>, why: &str) -> SqlReview {
    match hold_for(database, kind) {
        SqlReview::Hold { verb, targets, .. } => SqlReview::Hold {
            reason: format!(
                "{AIRHOUSE_TOO_OLD} ({why}); `{database}` is not written in a workspace preview, \
                 nothing was sent"
            ),
            verb,
            targets,
        },
        other => other,
    }
}

/// How a held managed-Airhouse write reads in the run report.
pub(crate) const AIRHOUSE_TOO_OLD: &str = "Airhouse < 0.1.49: preview writes held";

#[async_trait]
impl WorkspaceContext for PreviewPlatformContext {
    /// `None`: a preview has no working copy, on any node.
    fn workspace_path(&self) -> Option<&Path> {
        None
    }

    /// The staging revision's context, materialised. With none compiled, an
    /// empty directory — never the working copy or the process's cwd.
    async fn context_root(&self) -> ContextRoot {
        let manager = &self.inner.workspace_manager().config_manager;
        let materialised = self
            .pinned(crate::server::api::semantic_scan::materialise_agent_context(manager))
            .await;
        match materialised {
            Ok(Some(m)) => {
                let root = m.root.clone();
                ContextRoot::materialised(root, Box::new(m), self.scope.revision_id)
            }
            other => {
                if let Err(e) = other {
                    tracing::warn!(target: "preview", error = ?e, "preview context unavailable");
                }
                empty_root(self.scope.revision_id)
            }
        }
    }

    fn database_configs(&self) -> Vec<oxy_airlayer_compat::DatabaseConfig> {
        self.inner.database_configs()
    }

    async fn get_connector(&self, name: &str) -> Result<Arc<dyn DatabaseConnector>, String> {
        self.held_connector(name).await
    }

    /// The previewed branch's engine for `name`, scoped to this run or not.
    /// A preview swaps a database's credential, never its engine.
    fn string_literal(&self, database: &str) -> Option<StringLiteral> {
        let name = self.own_name(database).ok()?;
        self.database_type(name)
            .ok()
            .map(|engine| string_literal_of(&engine))
    }

    async fn get_integration(&self, name: &str) -> Result<IntegrationConfig, String> {
        self.pinned(self.inner.get_integration(name)).await
    }

    async fn fetch_secret(&self, name: &str) -> Option<String> {
        if self.secret_withheld(name) {
            return None;
        }
        self.inner.fetch_secret(name).await
    }

    async fn store_secret(&self, name: &str, _value: &str) -> Result<(), String> {
        Err(format!(
            "`{name}` is not written: a workspace preview persists no secrets"
        ))
    }

    /// The managed Airhouse's writes land in the preview's own schemas
    /// ([`super::airhouse_writes`]), or are held where that Airhouse cannot
    /// confine a preview Writer. On every other database, every statement
    /// that is not a read is held.
    async fn review_sql(&self, database: &str, sql: &str) -> Result<SqlReview, String> {
        let name = self.own_name(database)?;
        let kinds = classify(self.dialect_of(name)?, sql);
        if let Some(airhouse) = self.preview_airhouse(name)? {
            return Ok(match airhouse.review(sql, !is_all_read(&kinds)).await? {
                Step::Review(review) => review,
                Step::Held(why) => held_airhouse(name, first_non_read(&kinds), &why),
            });
        }
        if is_all_read(&kinds) {
            return Ok(SqlReview::Proceed);
        }
        Ok(hold_for(name, first_non_read(&kinds)))
    }

    /// A scoped method ([`names`]) is always a write — this platform scoped it
    /// for a writing verb or a `persist_to_secret` — so it is held whatever the
    /// verb; an unscoped one proceeds only for `GET`/`HEAD`.
    async fn review_http(&self, method: &str, _url: &str) -> HttpReview {
        use agentic_automation::preview_names::parse_scoped_method;
        let reason = match parse_scoped_method(method) {
            Some((run, _)) if !run.eq_ignore_ascii_case(&self.scope.run_id) => {
                "the request is scoped to another preview run".to_string()
            }
            Some((_, verb)) => format!(
                "a {verb} request (or one that persists a secret) is not sent in a workspace \
                 preview; only GET and HEAD are"
            ),
            None if matches!(method, "GET" | "HEAD") => return HttpReview::Proceed,
            None => format!(
                "a {method} request is not sent in a workspace preview; only GET and HEAD are"
            ),
        };
        HttpReview::Hold { reason }
    }

    async fn list_automation_files(&self) -> Result<Vec<PathBuf>, String> {
        self.pinned(self.inner.list_automation_files()).await
    }

    /// The staging revision's automation, re-emitted with preview-scoped names
    /// ([`names`]). Never the working copy.
    async fn resolve_automation_yaml(
        &self,
        automation_ref: &str,
    ) -> Result<String, WorkspaceReadError> {
        let name = self
            .own_name(automation_ref)
            .map_err(WorkspaceReadError::Missing)?;
        let manager = &self.inner.workspace_manager().config_manager;
        let found = self.pinned(manager.automation_definition(name)).await;
        let mut definition = match found {
            Ok(Some(definition)) => definition,
            Ok(None) => {
                return Err(WorkspaceReadError::Missing(format!(
                    "`{name}` is not in the previewed revision {}",
                    self.scope.revision_id
                )));
            }
            Err(e) if e.retryable() => return Err(WorkspaceReadError::Unavailable(e.to_string())),
            Err(e) => return Err(WorkspaceReadError::Invalid(e.to_string())),
        };
        names::scope_automation(&mut definition, &self.scope.run_id);
        serde_yaml::to_string(&definition).map_err(|e| WorkspaceReadError::Invalid(e.to_string()))
    }

    fn refresh_key_cache(
        &self,
    ) -> Option<Arc<RwLock<agentic_semantic::refresh_key_cache::RefreshKeyCache>>> {
        None
    }

    fn preagg_renewal_threshold_secs(&self) -> u64 {
        self.inner.preagg_renewal_threshold_secs()
    }

    /// `None`: rollups are built from the promoted model, not the branch's.
    fn preagg_workspace_id(&self) -> Option<uuid::Uuid> {
        None
    }

    fn preagg_blob(&self) -> Option<agentic_semantic::BlobConfig> {
        None
    }

    fn preagg_require_fresh(&self) -> bool {
        true
    }

    /// `None`: the shared cache is keyed by workspace; a branch's engine must
    /// not sit beside production's.
    fn semantic_engine_cache(
        &self,
    ) -> Option<(Arc<oxy_airlayer_compat::SemanticEngineCache>, uuid::Uuid)> {
        None
    }

    fn preagg_context(&self) -> Option<agentic_semantic::PreaggContext> {
        None
    }

    async fn list_airway_files(&self) -> Result<Vec<PathBuf>, String> {
        self.pinned(self.inner.list_airway_files()).await
    }

    async fn resolve_pipeline_yaml(&self, pipeline_ref: &str) -> Result<Option<String>, String> {
        let name = self.own_name(pipeline_ref)?;
        self.pinned(self.inner.resolve_pipeline_yaml(name)).await
    }

    fn compiled_revision(&self) -> Option<uuid::Uuid> {
        Some(self.scope.revision_id)
    }

    /// `false`, never asked: a preview reads its staging revision and holds
    /// writes, and a compile would mint and promote a `main` revision of the
    /// workspace it is previewing.
    async fn request_compile(&self) -> bool {
        false
    }

    async fn resolve_sql_file(&self, sql_ref: &str) -> Result<Option<String>, String> {
        self.pinned(self.inner.resolve_sql_file(sql_ref)).await
    }
}

/// An empty materialised root: agent context globs resolve to nothing.
fn empty_root(revision_id: uuid::Uuid) -> ContextRoot {
    match tempfile::tempdir() {
        Ok(dir) => {
            let path = dir.path().to_path_buf();
            ContextRoot::materialised(path, Box::new(dir), revision_id)
        }
        // No temp dir at all: a path that does not exist resolves no globs.
        Err(_) => ContextRoot::fs(PathBuf::from(super::NO_WORKING_COPY)),
    }
}
