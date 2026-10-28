use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use uuid::Uuid;

use crate::config::constants::DATABASE_SEMANTIC_PATH;
use oxy_shared::errors::OxyError;

use super::{
    artifacts::{
        AgentEntry, ArtifactError, AutomationEntry, CompiledArtifact, ContextDocuments,
        PipelineEntry, SimulationEntry, VerifiedQueryEntry,
    },
    model::{Automation, AutomationWithRawVariables, BuilderAgentConfig, Config, Database, Model},
    storage::{ConfigStorage, FsStorage},
    test_config::TestFileConfig,
};

// ── types ───────────────────────────────────────────────────────────────

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::WorkingCopy {}
    impl Sealed for super::ReadOnly {}
}

/// Two inhabitants:
///
/// - [`WorkingCopy`] — the process HAS the files. Unlocks the reads with no
///   compiled equivalent, the raw paths, and the writes.
/// - [`ReadOnly`] — the handler does not REQUIRE the files. It may still see
///   them: whether this NODE has them is a runtime fact, so the slot carries an
///   `Option` rather than asserting either way.
///
/// Why the second one cannot simply be "no working copy": one handler has one
/// signature, and it runs on both an ide (files present) and a serve replica
/// (files absent). A slot that asserts emptiness is a lie on the ide — which is
/// exactly what shipped: the middleware DOWNGRADED a manager holding the files,
/// keeping its `Origin::Disk`, so every boundary-miss fallback took the empty
/// arm. An unpromoted workspace, and every feature-branch preview, answered
/// `NoSource` for files sitting right there.
///
/// The pair `(slot, origin)` has three valid states, and the builder refuses
/// the fourth:
///
/// ```text
/// WorkingCopy + Compiled   promoted, on the node that owns the files
/// WorkingCopy + Disk       feature branch or not yet compiled — read the files
/// ReadOnly    + Compiled   a replica, serving the boundary
/// ReadOnly(None) + Disk    REFUSED — "read the files" with no files to read
/// ```
pub trait DiskSlot: sealed::Sealed + std::fmt::Debug + Clone {
    fn as_working_copy(&self) -> Option<&WorkingCopy>;
}

impl DiskSlot for WorkingCopy {
    fn as_working_copy(&self) -> Option<&WorkingCopy> {
        Some(self)
    }
}

/// A working copy when the process owns one. The slot a read-only handler gets.
#[derive(Debug, Clone)]
pub struct ReadOnly(Option<WorkingCopy>);

impl ReadOnly {
    /// Nothing to read. For a caller that genuinely has no working copy — a
    /// worker, a public webhook — as opposed to one narrowing a manager it
    /// holds, which is [`ConfigManager::into_read_only`].
    pub fn empty() -> Self {
        Self(None)
    }
}

impl DiskSlot for ReadOnly {
    fn as_working_copy(&self) -> Option<&WorkingCopy> {
        self.0.as_ref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Disk,
    Compiled {
        workspace_id: Uuid,
        revision_id: Uuid,
    },
}

#[derive(Debug, Clone)]
pub struct WorkingCopy {
    root: PathBuf,
    state_dir: PathBuf,
    storage: Arc<FsStorage>,
}

impl WorkingCopy {
    pub(super) fn new(storage: FsStorage) -> Self {
        let root = storage.project_path().to_path_buf();
        let state_dir = crate::state_dir::state_dir_path(Some(root.join(".oxy_state")));
        Self {
            root,
            state_dir,
            storage: Arc::new(storage),
        }
    }

    pub(super) fn storage(&self) -> &FsStorage {
        &self.storage
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }
}

#[derive(Debug, Clone)]
pub struct ConfigManager<S> {
    config: Arc<Config>,
    source: S,
    origin: Origin,
}

pub trait ResolveWorkspaceFile {
    fn try_resolve_file(
        &self,
        file_ref: &str,
    ) -> impl std::future::Future<Output = Result<String, OxyError>> + Send;

    fn workspace_file_resolver(&self) -> Option<ConfigManager<WorkingCopy>>;
}

impl ResolveWorkspaceFile for ConfigManager<WorkingCopy> {
    async fn try_resolve_file(&self, file_ref: &str) -> Result<String, OxyError> {
        self.resolve_file(file_ref).await
    }

    fn workspace_file_resolver(&self) -> Option<ConfigManager<WorkingCopy>> {
        Some(self.clone())
    }
}

impl ResolveWorkspaceFile for ConfigManager<ReadOnly> {
    async fn try_resolve_file(&self, file_ref: &str) -> Result<String, OxyError> {
        match self.workspace_file_resolver() {
            Some(manager) => manager.resolve_file(file_ref).await,
            None => Err(OxyError::ConfigurationError(format!(
                "`{file_ref}` is a path inside the workspace working copy, which \
                 this process does not have."
            ))),
        }
    }

    /// The escalation: a read-only manager hands back a full one when the pod
    /// actually holds the files. This is what "use the disk if there is one"
    /// means concretely — the capability is a runtime fact about the NODE, and
    /// the type only says whether the handler REQUIRES it.
    ///
    /// "Actually holds" is the `is_dir` check, not the slot. Without it this
    /// escalated on every replica — the slot is always full there — and
    /// `resolve_file` then failed inside `canonicalize`, so the caller got a
    /// raw `IOError` about a path it never wrote instead of the sentence three
    /// lines up that names the real condition.
    fn workspace_file_resolver(&self) -> Option<ConfigManager<WorkingCopy>> {
        self.source
            .as_working_copy()
            .filter(|wc| wc.root().is_dir())
            .map(|wc| ConfigManager {
                config: self.config.clone(),
                source: wc.clone(),
                origin: self.origin,
            })
    }
}

// ── 1. the in-memory Config — no I/O, no capability ─────────────────────

impl<S> ConfigManager<S> {
    pub(super) fn new(config: Config, source: S, origin: Origin) -> Self {
        Self {
            config: Arc::new(config),
            source,
            origin,
        }
    }

    pub fn origin(&self) -> Origin {
        self.origin
    }

    pub fn revision_id(&self) -> Option<Uuid> {
        match self.origin {
            Origin::Compiled { revision_id, .. } => Some(revision_id),
            Origin::Disk => None,
        }
    }

    pub fn get_config(&self) -> &Config {
        &self.config
    }

    pub fn models(&self) -> &[Model] {
        &self.config.models
    }

    pub fn resolve_model(&self, model_name: &str) -> Result<&Model, OxyError> {
        let model = self
            .config
            .models
            .iter()
            .find(|m| m.name() == model_name)
            .ok_or_else(|| {
                OxyError::ConfigurationError(format!("Model '{model_name}' not found in config"))
            })?;
        Ok(model)
    }

    pub fn default_model(&self) -> Option<&str> {
        self.config.models.first().map(|m| m.name())
    }

    pub fn get_model_key_var(&self, model: &Model) -> Option<String> {
        model.key_var().map(|s| s.to_string())
    }

    pub fn list_databases(&self) -> Vec<Database> {
        self.config.databases.clone()
    }

    pub fn resolve_database(&self, database_name: &str) -> Result<Database, OxyError> {
        self.config
            .databases
            .iter()
            .find(|w| w.name == database_name)
            .cloned()
            .ok_or_else(|| {
                OxyError::ConfigurationError(format!(
                    "Database '{database_name}' not found in config"
                ))
            })
    }

    pub fn default_database_ref(&self) -> Option<&String> {
        self.config.defaults.as_ref().map(|d| d.database.as_ref())?
    }

    pub fn get_database_password_var(&self, database: &Database) -> Option<String> {
        match &database.database_type {
            crate::config::model::DatabaseType::Postgres(postgres) => postgres.password_var.clone(),
            crate::config::model::DatabaseType::Airhouse(airhouse) => airhouse.password_var.clone(),
            crate::config::model::DatabaseType::Mysql(mysql) => mysql.password_var.clone(),
            crate::config::model::DatabaseType::Snowflake(snowflake) => {
                snowflake.auth_type.get_password_var().cloned()
            }
            crate::config::model::DatabaseType::ClickHouse(clickhouse) => {
                clickhouse.password_var.clone()
            }
            crate::config::model::DatabaseType::Redshift(redshift) => redshift.password_var.clone(),
            _ => None,
        }
    }

    pub fn get_integration_by_name(
        &self,
        integration_name: &str,
    ) -> Option<&crate::config::model::Integration> {
        self.config
            .integrations
            .iter()
            .find(|i| i.name == integration_name)
    }

    pub fn list_looker_integrations(&self) -> Vec<&super::model::Integration> {
        self.config
            .integrations
            .iter()
            .filter(|i| matches!(i.integration_type, super::model::IntegrationType::Looker(_)))
            .collect()
    }

    pub fn toast_analytics_integrations(
        &self,
    ) -> Vec<(&str, &super::model::ToastAnalyticsIntegration)> {
        self.config
            .integrations
            .iter()
            .filter_map(|i| match &i.integration_type {
                super::model::IntegrationType::ToastAnalytics(t) => Some((i.name.as_str(), t)),
                _ => None,
            })
            .collect()
    }

    pub fn get_builder_config(&self) -> Option<&BuilderAgentConfig> {
        self.config.builder_agent.as_ref()
    }

    pub fn is_builder_builtin(&self) -> bool {
        matches!(
            self.config.builder_agent,
            Some(BuilderAgentConfig::Builtin { .. })
        )
    }

    pub fn protected_branches(&self) -> Option<&[String]> {
        self.config.protected_branches.as_deref()
    }

    pub fn base_branch(&self) -> Option<&str> {
        self.config.base_branch.as_deref()
    }

    pub fn timezone(&self) -> Option<&str> {
        self.config.timezone.as_deref()
    }
}

// ── 2. works on either capability — asks whether a disk is there ────────

impl<S: DiskSlot> ConfigManager<S> {
    pub fn working_copy(&self) -> Option<&WorkingCopy> {
        self.source.as_working_copy()
    }

    pub fn can_read_disk(&self) -> bool {
        self.working_copy().is_some() && crate::workspace_fs_probe::process_owns_workspace_files()
    }

    /// The working copy, or the reason there is nothing to read.
    ///
    /// The absent-root check is here rather than left to `require_root()` so the
    /// two failures stay distinguishable to a caller: a root that is not on this
    /// node is transient (mid-deploy, not cloned yet) and retryable, while a
    /// file that fails to parse is a real fault. Collapsing them turns a 503
    /// into a 500.
    fn disk(&self) -> Result<&FsStorage, ArtifactError> {
        let working_copy = self.working_copy().ok_or(ArtifactError::NoSource)?;
        if !working_copy.root().is_dir() {
            return Err(ArtifactError::WorkspaceUnavailable(
                working_copy.root().display().to_string(),
            ));
        }
        Ok(working_copy.storage())
    }

    /// The compiled semantic views and topics of the revision this manager
    /// reads, for a caller that wants the ROWS and specifically wants them from
    /// the compile boundary.
    ///
    /// The only such caller is the workspace-health smoke test, which probes
    /// what was promoted — asking it about the working copy would answer a
    /// different question. `Origin::Disk` is therefore an error here, not an
    /// empty list.
    ///
    /// Everything that wants these artifacts in order to READ them wants a
    /// directory instead: [`Self::semantics_scan`] and [`Self::agent_context`],
    /// which own the compiled-vs-disk choice rather than returning a `None`
    /// for the caller to interpret.
    pub async fn promoted_semantic_entities(
        &self,
    ) -> Result<(Vec<CompiledArtifact>, Vec<CompiledArtifact>), ArtifactError> {
        let (Some(views), Some(topics)) = (
            self.compiled_semantic_views().await?,
            self.compiled_semantic_topics().await?,
        ) else {
            return Err(ArtifactError::NoSource);
        };
        Ok((views, topics))
    }

    /// The compiled semantic views, or `None` when this manager reads the
    /// working copy.
    ///
    /// Crate-private on purpose: `None` here means "not reading a compiled
    /// revision", which is half a decision, and every public caller that had to
    /// interpret it got the other half slightly different. The two that resolve
    /// it are in `scan.rs`.
    pub(super) async fn compiled_semantic_views(
        &self,
    ) -> Result<Option<Vec<CompiledArtifact>>, ArtifactError> {
        match self.origin {
            Origin::Compiled { revision_id, .. } => {
                super::compiled::list_semantic_views_at(revision_id)
                    .await
                    .map(Some)
            }
            Origin::Disk => Ok(None),
        }
    }

    /// See [`Self::compiled_semantic_views`].
    pub(super) async fn compiled_semantic_topics(
        &self,
    ) -> Result<Option<Vec<CompiledArtifact>>, ArtifactError> {
        match self.origin {
            Origin::Compiled { revision_id, .. } => {
                super::compiled::list_semantic_topics_at(revision_id)
                    .await
                    .map(Some)
            }
            Origin::Disk => Ok(None),
        }
    }

    /// The compiled automations with their full bodies — what an agent's
    /// `context:` glob needs, as opposed to the listing rows.
    pub(super) async fn compiled_automation_artifacts(
        &self,
    ) -> Result<Option<Vec<CompiledArtifact>>, ArtifactError> {
        match self.origin {
            Origin::Compiled { revision_id, .. } => {
                super::compiled::list_automation_artifacts_at(revision_id)
                    .await
                    .map(Some)
            }
            Origin::Disk => Ok(None),
        }
    }

    /// The compiled verified queries (`.sql`).
    pub(super) async fn compiled_verified_queries(
        &self,
    ) -> Result<Option<Vec<VerifiedQueryEntry>>, ArtifactError> {
        match self.origin {
            Origin::Compiled { revision_id, .. } => {
                super::compiled::list_verified_queries_at(revision_id)
                    .await
                    .map(Some)
            }
            Origin::Disk => Ok(None),
        }
    }

    /// The directory `airlayer` scans for `.view.yml` / `.topic.yml` on a node
    /// that holds the working copy. The whole project root, so semantic files
    /// can live anywhere under it.
    /// Where airlayer scans from, when this node holds the files.
    ///
    /// `disk()`, not `working_copy()`. The slot being full is not the files
    /// being there: `effective_workspace_path` hands back the database column
    /// without stat-ing it and `ReadOnly` keeps the slot, so on a replica this
    /// asked "is there a handle?" — always yes — and returned a directory that
    /// is not on the node. Airlayer then scanned it.
    ///
    /// It also made `scan_dir`'s empty-revision arm unreachable, which is the
    /// arm written FOR that node: the permanent 503 it was meant to close
    /// stayed open, and the test that covered it used a slot-less manager, a
    /// shape production never produces.
    pub fn semantics_scan_dir(&self) -> Result<PathBuf, ArtifactError> {
        Ok(self.disk()?.project_path().to_path_buf())
    }

    /// One workspace YAML file, parsed, from the working copy.
    ///
    /// The compiled `definition` column IS this value: `oxy_compile` stores
    /// `serde_yaml::from_str::<Value>(content)` with no transformation, so the
    /// two arms of every `*_definition` method below are the same shape by
    /// construction rather than by convention.
    ///
    /// `Ok(None)` means the file is not there. That is the ONLY absence this
    /// returns — a missing working copy is `Err` from `disk()`, because "there
    /// is nothing to look in" and "I looked and found nothing" are different
    /// answers and only one of them is the customer's.
    async fn definition_from_disk(
        &self,
        file_path: &str,
    ) -> Result<Option<serde_json::Value>, ArtifactError> {
        // `fs_link` resolves and validates containment — it returns the PATH,
        // not the bytes. Going through it rather than joining onto the root is
        // what keeps `../` out; `nothing_new_takes_a_raw_workspace_path` exists
        // because that shortcut was taken once.
        let resolved = self.disk()?.fs_link(file_path).await?;
        let content = match tokio::fs::read_to_string(&resolved).await {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            // `WorkspaceUnavailable`, not `Config(IOError)`. `retryable()` is
            // `!matches!(self, Config(_))`, and callers turn non-retryable into
            // "the customer's YAML is bad" — a 400 in `list_monitors`, a 422 in
            // `get_automation_file`. A permission error, an EIO, a volume
            // half-mounted are none of those. `root_singleton` below already
            // draws this line for the same read; this is the sibling that did
            // not, and the 422 was built on top of it.
            Err(e) => {
                return Err(ArtifactError::WorkspaceUnavailable(format!(
                    "`{file_path}` exists but could not be read: {e}"
                )));
            }
        };
        Ok(Some(
            serde_yaml::from_str::<serde_json::Value>(&content)
                .map_err(|e| OxyError::ConfigurationError(format!("{file_path}: {e}")))?,
        ))
    }

    /// One definition for a kind that is read from the promoted revision and
    /// from nothing else once a revision is pinned: automations and airway
    /// pipelines.
    ///
    /// There is no working-copy fallback under `Origin::Compiled`, on any node. A miss is the revision's answer ("not in
    /// this revision") and a failed read is an error the caller can retry; a
    /// node that happens to hold the files does not get to answer instead,
    /// because then the same request returns different YAML depending on which
    /// pod served it, and the files are what is being retired
    /// (`internal-docs/factory-retirement.md`, Phase 4).
    ///
    /// `Origin::Disk` is untouched. It means this manager was built to read a
    /// directory (the CLI, single-process mode, a draft branch in the IDE), and
    /// no revision was ever pinned for it to defer to.
    async fn revision_definition(
        &self,
        compiled: impl Future<Output = Result<Option<serde_json::Value>, ArtifactError>>,
        file_path: &str,
    ) -> Result<Option<serde_json::Value>, ArtifactError> {
        match self.origin {
            Origin::Compiled { .. } => compiled.await,
            Origin::Disk => self.definition_from_disk(file_path).await,
        }
    }

    /// One automation's definition, from whichever source this manager reads.
    ///
    /// Returns the raw JSON rather than a parsed type because the callers want
    /// it that way — one re-serialises it to YAML for the diagram, one reads a
    /// single key. [`Self::resolve_automation`] gives the parsed `Automation`.
    pub async fn automation_definition(
        &self,
        file_path: &str,
    ) -> Result<Option<serde_json::Value>, ArtifactError> {
        let compiled = async move {
            match self.origin {
                Origin::Compiled { revision_id, .. } => {
                    super::compiled::resolve_automation_at(revision_id, file_path).await
                }
                Origin::Disk => Ok(None),
            }
        };
        self.revision_definition(compiled, file_path).await
    }

    /// One airway pipeline's definition, from whichever source this manager
    /// reads. Same shape as [`Self::automation_definition`].
    pub async fn pipeline_definition(
        &self,
        file_path: &str,
    ) -> Result<Option<serde_json::Value>, ArtifactError> {
        let compiled = async move {
            match self.origin {
                Origin::Compiled { revision_id, .. } => {
                    super::compiled::resolve_pipeline_at(revision_id, file_path).await
                }
                Origin::Disk => Ok(None),
            }
        };
        self.revision_definition(compiled, file_path).await
    }

    /// One verified query's SQL body, from whichever source this manager reads.
    ///
    /// Unlike the `*_definition` methods this returns the raw SQL rather than a
    /// JSONB `definition`, so it does not go through [`Self::revision_definition`] —
    /// that helper's disk arm reads and parses YAML. The disk arm here is the
    /// caller's existing filesystem read, which stays where it is: this method
    /// answers only "is it in the compile boundary", and `Ok(None)` means
    /// "ask the working copy".
    ///
    /// `Err` is deliberately NOT laundered into `Ok(None)`. A backend failure is
    /// "could not look", and a caller that treated it as "not compiled" would
    /// fall through to a filesystem this node may not have — the instance
    /// affinity bug the compile boundary exists to remove.
    pub async fn verified_query_content(
        &self,
        file_path: &str,
    ) -> Result<Option<String>, ArtifactError> {
        match self.origin {
            Origin::Compiled { revision_id, .. } => {
                super::compiled::resolve_verified_query_at(revision_id, file_path).await
            }
            Origin::Disk => Ok(None),
        }
    }

    /// One analytics agent's definition, keyed by NAME rather than by path.
    ///
    /// The name is the compile boundary's key, so the disk arm has to translate
    /// it back: `list_analytics_agents` derives the same name from the same
    /// path (`artifact_name`, pinned against `oxy_compile` by
    /// `crates/app/tests/platform/artifact_naming_agrees.rs`), which is what makes the
    /// two arms answer about the same file.
    pub async fn agent_definition(
        &self,
        name: &str,
    ) -> Result<Option<serde_json::Value>, ArtifactError> {
        if let Origin::Compiled { revision_id, .. } = self.origin {
            match super::compiled::resolve_agent_at(revision_id, name).await {
                Ok(Some(value)) => return Ok(Some(value)),
                Ok(None) => {}
                Err(e) => match self.working_copy() {
                    Some(_) => tracing::warn!(
                        name,
                        error = %e,
                        "compile boundary failed; falling back to the working copy"
                    ),
                    None => return Err(e),
                },
            }
        }
        let Some(entry) = self
            .disk()?
            .list_analytics_agents()
            .await?
            .into_iter()
            .find(|a| a.name == name)
        else {
            return Ok(None);
        };
        self.definition_from_disk(&entry.file_path).await
    }

    /// The workspace's `.monitor.yml`, compiled or from disk.
    pub async fn monitor_config(&self) -> Result<Option<serde_json::Value>, ArtifactError> {
        self.root_singleton(super::compiled::resolve_monitor_config_at, ".monitor.yml")
            .await
    }

    /// The workspace's `reconcile.yml`, compiled or from disk.
    pub async fn reconcile_config(&self) -> Result<Option<serde_json::Value>, ArtifactError> {
        self.root_singleton(
            super::compiled::resolve_reconcile_config_at,
            "reconcile.yml",
        )
        .await
    }

    /// The workspace's `.world-model.yml`, compiled or from disk.
    ///
    /// On a pod that cannot read a disk the pinned revision is the only source,
    /// so its answer is final: no row means the workspace declares none, and a
    /// failed read is the boundary's own error. [`Self::root_singleton`] reports
    /// both as the missing working copy, which suits a caller that only needs
    /// "could not look" and is useless to one that must tell a workspace with
    /// no display config from a read that failed.
    pub async fn world_model_config(&self) -> Result<Option<serde_json::Value>, ArtifactError> {
        if let Origin::Compiled { revision_id, .. } = self.origin
            && !self.can_read_disk()
        {
            return super::compiled::resolve_world_model_config_at(revision_id).await;
        }
        self.root_singleton(
            super::compiled::resolve_world_model_config_at,
            ".world-model.yml",
        )
        .await
    }

    /// A root-level singleton file: one per workspace, one row per revision.
    ///
    /// `Ok(None)` means the workspace declares none — every one of these is
    /// opt-in, so absence is the common case and must stay distinct from an
    /// error, which means we could not find out. That distinction is the whole
    /// reason a replica with no compiled row returns `NoSource` here rather
    /// than `None`.
    async fn root_singleton<F, Fut>(
        &self,
        compiled: F,
        file_name: &str,
    ) -> Result<Option<serde_json::Value>, ArtifactError>
    where
        F: FnOnce(Uuid) -> Fut,
        Fut: Future<Output = Result<Option<serde_json::Value>, ArtifactError>>,
    {
        if let Origin::Compiled { revision_id, .. } = self.origin {
            match compiled(revision_id).await {
                Ok(Some(definition)) => return Ok(Some(definition)),
                Ok(None) => {}
                Err(e) => match self.working_copy() {
                    Some(_) => tracing::warn!(
                        file_name,
                        error = %e,
                        "compile boundary failed; falling back to the working copy"
                    ),
                    None => return Err(e),
                },
            }
        }
        let path = self.disk()?.project_path().join(file_name);
        match tokio::fs::read_to_string(&path).await {
            Ok(yaml) => Ok(Some(serde_yaml::from_str(&yaml).map_err(|e| {
                ArtifactError::Config(OxyError::ConfigurationError(format!(
                    "`{file_name}` did not parse: {e}"
                )))
            })?)),
            // Only a genuinely absent file is `None`. A permission error, an
            // EIO, a truncated mount all used to arrive here as "the workspace
            // declares no `{file_name}`" — the exact conflation the compiled
            // arm above already refuses.
            //
            // `WorkspaceUnavailable` rather than `Config(IOError)` because
            // `retryable()` is `!matches!(self, Config(_))`, and
            // `list_monitors` turns non-retryable into a 400 on the grounds
            // that it is the customer's YAML. A disk fault is not.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ArtifactError::WorkspaceUnavailable(format!(
                "`{file_name}` exists but could not be read: {e}"
            ))),
        }
    }

    /// Every automation.
    ///
    /// Under `Origin::Compiled` this reads the revision and nothing else: a failed
    /// read is the error, never a reason to list the working copy instead.
    /// `Origin::Disk` lists the directory it was built over.
    ///
    /// The two arms do NOT enumerate the same set and this does not change
    /// that: the working copy unions `.procedure.yml`, `.workflow.yml` and
    /// `.automation.yml`, while the walker dropped `.workflow.yml` as a file
    /// kind. So a `.workflow.yml` automation is listed on a workspace read from
    /// disk and absent on one read from the boundary — exactly as before this
    /// moved. `automations_diverge_on_the_legacy_workflow_extension` pins it.
    /// Closing that gap means deciding whether a no-longer-recognised extension
    /// should still appear, which is a product question, not a refactor.
    pub async fn list_automations(&self) -> Result<Vec<AutomationEntry>, ArtifactError> {
        match self.origin {
            Origin::Compiled { revision_id, .. } => {
                super::compiled::list_automations_at(revision_id).await
            }
            Origin::Disk => Ok(self.disk()?.list_workflows().await?),
        }
    }

    /// Every analytics agent. Both arms drop a file whose NAME contains
    /// `.test.` — the walker in `oxy_compile::walker::discover`, the working
    /// copy in `storage.rs`'s `list_entity_files` — so the enumerations agree
    /// on both spellings: the conventional `x.agent.test.yml` (which ends
    /// `.test.yml`, not `.agentic.yml`, so no extension match claims it
    /// either way) and `x.test.agentic.yml` (which DOES end `.agentic.yml`,
    /// and which only the shared `.test.` rule keeps off both lists).
    /// `agents_agree_on_the_test_mirror_convention` pins that.
    pub async fn list_analytics_agents(&self) -> Result<Vec<AgentEntry>, ArtifactError> {
        match self.origin {
            Origin::Compiled { revision_id, .. } => {
                match super::compiled::list_agents_at(revision_id).await {
                    Ok(agents) => Ok(agents),
                    Err(e) => match self.working_copy() {
                        Some(_) => {
                            tracing::warn!(
                                error = %e,
                                "compile boundary failed; falling back to the working copy"
                            );
                            Ok(self.disk()?.list_analytics_agents().await?)
                        }
                        None => Err(e),
                    },
                }
            }
            Origin::Disk => Ok(self.disk()?.list_analytics_agents().await?),
        }
    }

    /// Every Airway pipeline. Under `Origin::Compiled` this reads the revision and
    /// nothing else, as [`Self::list_automations`] does. The two arms enumerate the same set — the walker
    /// and the working copy agree on `.airway.yml` — so this is the one kind
    /// where the compiled and disk answers are interchangeable by construction.
    pub async fn list_pipelines(&self) -> Result<Vec<PipelineEntry>, ArtifactError> {
        match self.origin {
            Origin::Compiled { revision_id, .. } => {
                super::compiled::list_pipelines_at(revision_id).await
            }
            Origin::Disk => Ok(self.disk()?.list_pipelines().await?),
        }
    }

    /// Every declared world (`.simulation.yml`). The two arms enumerate the
    /// same set — the walker and the working copy agree on the extension, on
    /// the skip set, AND on the `.test.` file-name rule — and both carry the
    /// body, because the grid renders off it and a run reads its seed out of
    /// it. Each of those three is exactly one definition shared by both arms:
    /// `oxy_compile::walker::is_skipped` for directories (called by the
    /// compile walker and by `crates/core/src/config/storage.rs`'s
    /// `list_by_sub_extension`), and the `.test.` file-name test that the
    /// walker and `storage.rs`'s `list_entity_files` each apply. So neither a
    /// world under `build/` or a nested `sub/target/`, nor a
    /// `baseline.test.simulation.yml`, can exist on a replica and not the IDE
    /// (or vice versa) depending on which arm answered.
    ///
    /// A promoted revision carrying no worlds is authoritatively empty and does
    /// NOT fall through to the working copy: only a boundary FAILURE does, and
    /// only on a node that has one. Answering `[]` for a node that could not
    /// look is how "this replica holds no files" gets reported as "this
    /// workspace declares no worlds".
    pub async fn list_simulations(&self) -> Result<Vec<SimulationEntry>, ArtifactError> {
        match self.origin {
            Origin::Compiled { revision_id, .. } => {
                match super::compiled::list_simulations_at(revision_id).await {
                    Ok(simulations) => Ok(simulations),
                    Err(e) => match self.working_copy() {
                        Some(_) => {
                            tracing::warn!(
                                error = %e,
                                "compile boundary failed; falling back to the working copy"
                            );
                            Ok(self.disk()?.list_simulations().await?)
                        }
                        None => Err(e),
                    },
                }
            }
            Origin::Disk => Ok(self.disk()?.list_simulations().await?),
        }
    }

    /// One declared world's definition, keyed by NAME rather than by path —
    /// same reasoning as [`Self::agent_definition`]: the name is the compile
    /// boundary's key, and a run references the world it is running, so the
    /// reference has to survive the file being moved.
    ///
    /// The disk arm translates the name back through
    /// [`Self::list_simulations`], which derives it with the same rule the
    /// walker uses (`artifact_name`, pinned by
    /// `crates/app/tests/platform/artifact_naming_agrees.rs`) — that is what makes the
    /// two arms answer about the same file. It already carries the body, so
    /// unlike `agent_definition` there is no second read.
    pub async fn simulation_definition(
        &self,
        name: &str,
    ) -> Result<Option<serde_json::Value>, ArtifactError> {
        if let Origin::Compiled { revision_id, .. } = self.origin {
            match super::compiled::resolve_simulation_at(revision_id, name).await {
                Ok(Some(value)) => return Ok(Some(value)),
                Ok(None) => {}
                Err(e) => match self.working_copy() {
                    Some(_) => tracing::warn!(
                        name,
                        error = %e,
                        "compile boundary failed; falling back to the working copy"
                    ),
                    None => return Err(e),
                },
            }
        }
        Ok(self
            .disk()?
            .list_simulations()
            .await?
            .into_iter()
            .find(|s| s.name == name)
            .map(|s| s.definition))
    }

    /// The markdown documents an analytics agent's `context:` patterns reach,
    /// from whichever source this manager reads.
    ///
    /// `Origin` decides, as for every other artifact, and not the process
    /// role: a node that owns the files and serves a promoted revision reads
    /// the revision, the same one the agent's own definition came from.
    ///
    /// Three outcomes, and keeping them apart is the point:
    ///
    /// - `Ok(Read(documents))`, possibly empty: looked, and this is what the
    ///   agent reads. A pattern list that cannot name a `.md` at all is
    ///   answered here without reading anything.
    /// - `Ok(NotCompiled)`: the revision was compiled before documents were a
    ///   compiled kind, and this node has no files to read instead. Nobody
    ///   looked. See [`ContextDocuments::NotCompiled`] for why that is neither
    ///   an error nor an empty answer.
    /// - `Err`: could not look. A revision that DOES carry documents is
    ///   strict: a failed read of it is an error, never an empty list.
    ///
    /// An older revision on a node that holds the files reads them, which is
    /// exactly what that node did before documents were compiled.
    pub async fn context_documents(
        &self,
        patterns: &[String],
    ) -> Result<ContextDocuments, ArtifactError> {
        let patterns = oxy_compile::context_documents::ContextPatterns::new(patterns);
        if !patterns.may_reach_documents() {
            return Ok(ContextDocuments::Read(Vec::new()));
        }
        if let Origin::Compiled { revision_id, .. } = self.origin {
            match super::context_documents::documents_at(revision_id).await {
                Ok(Some(documents)) => {
                    let documents = super::context_documents::select(&patterns, documents);
                    return Ok(ContextDocuments::Read(documents));
                }
                // Both fallbacks ask `disk()`, not `working_copy()`. The slot
                // is full on a replica too (it names a directory that is not
                // there), so asking it would send a boundary fault on to a disk
                // read that fails, and report the fault as "workspace not on
                // this node" instead of what it was.
                Ok(None) if self.disk().is_err() => return Ok(ContextDocuments::NotCompiled),
                Ok(None) => tracing::debug!(
                    %revision_id,
                    "revision predates context documents; reading the working copy"
                ),
                Err(e) if self.disk().is_err() => return Err(e),
                Err(e) => tracing::warn!(
                    error = %e,
                    "compile boundary failed; falling back to the working copy"
                ),
            }
        }
        // Every failure of this listing is the node failing to look (`disk()`
        // has already established the root is here), so none of them is the
        // customer's configuration.
        let files = self
            .disk()?
            .list_context_documents(&patterns)
            .await
            .map_err(|e| ArtifactError::WorkspaceUnavailable(e.to_string()))?;
        Ok(ContextDocuments::Read(
            super::context_documents::read(files).await?,
        ))
    }

    /// The runtime state dir, created on the way out.
    ///
    /// The `is_dir` filter is the same guard `FsStorage::resolve_state_dir`
    /// already applies, and for the same reason: the fallback is
    /// `<root>/.oxy_state`, INSIDE the workspace root, so handing it a root
    /// this node does not have makes `create_dir_all` manufacture that root.
    ///
    /// The slot means "this manager has a working-copy handle", not "the files
    /// are here" — a replica gets one because `effective_workspace_path`
    /// returns the database column without stat-ing it. Closing that gap is
    /// what `disk()` is for, and this reaches past `disk()` straight to the
    /// handle, so it has to repeat the test.
    ///
    /// Without it, one `GET /charts/{file}` — a `FleetOk` route — left an empty
    /// root behind, and from then on `disk()`'s guard passed for that workspace
    /// on that node forever: `list_apps` answered `Ok([])`, "the customer
    /// configured nothing", from a node that never looked. `WorkingCopy::new`
    /// was already stopped from doing this; the creating resolver put it back
    /// one call later.
    pub fn runtime_state_dir(&self) -> PathBuf {
        crate::state_dir::resolve_state_dir_with_fallback(
            self.working_copy()
                .filter(|fs| fs.root().is_dir())
                .map(|fs| fs.state_dir().to_path_buf()),
        )
    }

    pub fn results_dir(&self) -> PathBuf {
        self.runtime_state_dir().join("results")
    }

    pub fn charts_dir(&self) -> PathBuf {
        self.runtime_state_dir().join("charts")
    }
}

// ── 3. requires a working copy — reads ──────────────────────────────────

impl ConfigManager<WorkingCopy> {
    /// Drop the REQUIREMENT for a working copy while keeping the working copy.
    ///
    /// The handler stops being able to call anything on this impl block —
    /// `workspace_path`, `resolve_file`, the writes — which is the point. What
    /// it keeps is the `impl<S: DiskSlot>` reads, whose disk arm is a
    /// documented fallback behind `disk()`. Throwing the slot away instead made
    /// those arms unreachable on the one node that could serve them.
    pub fn into_read_only(self) -> ConfigManager<ReadOnly> {
        ConfigManager {
            config: self.config,
            source: ReadOnly(Some(self.source)),
            origin: self.origin,
        }
    }

    /// Assert there is nothing to read, discarding a working copy this manager
    /// holds. Only tests want this — production narrowing is
    /// [`Self::into_read_only`], which keeps the slot.
    pub fn without_working_copy(self) -> ConfigManager<ReadOnly> {
        ConfigManager {
            config: self.config,
            source: ReadOnly::empty(),
            origin: self.origin,
        }
    }

    pub async fn list_tests(&self) -> Result<Vec<PathBuf>, OxyError> {
        self.source.storage().list_tests().await
    }

    pub async fn resolve_automation<P: AsRef<Path>>(
        &self,
        automation_name: P,
    ) -> Result<Automation, OxyError> {
        self.source
            .storage()
            .load_automation_config(automation_name)
            .await
    }

    pub async fn resolve_automation_with_raw_variables<P: AsRef<Path>>(
        &self,
        automation_name: P,
    ) -> Result<AutomationWithRawVariables, OxyError> {
        self.source
            .storage()
            .load_automation_config_with_raw_variables(automation_name)
            .await
    }

    pub async fn resolve_test<P: AsRef<Path>>(
        &self,
        test_ref: P,
    ) -> Result<TestFileConfig, OxyError> {
        self.source.storage().load_test_config(test_ref).await
    }

    pub async fn resolve_file<P: AsRef<Path>>(&self, file_ref: P) -> Result<String, OxyError> {
        self.source.storage().fs_link(file_ref).await
    }

    // ── paths into the working copy ──

    pub fn workspace_path(&self) -> &Path {
        self.source.root()
    }

    pub fn semantics_path(&self) -> PathBuf {
        self.source.root().join("semantics")
    }

    pub fn semantics_scan_path(&self) -> PathBuf {
        self.source.root().to_path_buf()
    }

    pub fn database_semantic_path(&self) -> PathBuf {
        self.source.root().join(DATABASE_SEMANTIC_PATH)
    }

    pub async fn resolve_state_dir(&self) -> Result<PathBuf, OxyError> {
        self.source.storage().resolve_state_dir().await
    }

    pub async fn get_charts_dir(&self) -> Result<PathBuf, OxyError> {
        self.source.storage().get_charts_dir().await
    }

    pub async fn get_results_dir(&self) -> Result<PathBuf, OxyError> {
        self.source.storage().get_results_dir().await
    }

    // ── writes to config.yml ──

    async fn config_for_write(&self) -> Config {
        match self.source.storage().load_config().await {
            Ok(fresh) => fresh,
            Err(e) => {
                tracing::debug!(error = %e, "no readable config.yml; writing from memory");
                (*self.config).clone()
            }
        }
    }

    pub async fn update_databases(&self, new_databases: Vec<Database>) -> Result<(), OxyError> {
        let mut updated_config = self.config_for_write().await;
        updated_config.databases = new_databases;

        self.source.storage().write_config(&updated_config).await?;
        Ok(())
    }

    pub async fn add_database(&self, database: Database) -> Result<(), OxyError> {
        let mut updated_config = self.config_for_write().await;

        if updated_config
            .databases
            .iter()
            .any(|db| db.name == database.name)
        {
            return Err(OxyError::ConfigurationError(format!(
                "Database with name '{}' already exists",
                database.name
            )));
        }

        updated_config.databases.push(database);
        self.source.storage().write_config(&updated_config).await?;
        Ok(())
    }

    pub async fn add_databases(&self, databases: Vec<Database>) -> Result<(), OxyError> {
        let mut updated_config = self.config_for_write().await;

        for database in &databases {
            if updated_config
                .databases
                .iter()
                .any(|db| db.name == database.name)
            {
                return Err(OxyError::ConfigurationError(format!(
                    "Database with name '{}' already exists",
                    database.name
                )));
            }
        }

        updated_config.databases.extend(databases);
        self.source.storage().write_config(&updated_config).await?;
        Ok(())
    }

    pub async fn remove_database(&self, database_name: &str) -> Result<(), OxyError> {
        let mut updated_config = self.config_for_write().await;

        let initial_len = updated_config.databases.len();
        updated_config
            .databases
            .retain(|db| db.name != database_name);

        if updated_config.databases.len() == initial_len {
            return Err(OxyError::ConfigurationError(format!(
                "Database with name '{}' not found",
                database_name
            )));
        }

        self.source.storage().write_config(&updated_config).await?;
        Ok(())
    }

    pub async fn remove_model(&self, model_name: &str) -> Result<(), OxyError> {
        let mut updated_config = self.config_for_write().await;

        let initial_len = updated_config.models.len();
        updated_config.models.retain(|m| m.name() != model_name);

        if updated_config.models.len() == initial_len {
            return Err(OxyError::ConfigurationError(format!(
                "Model with name '{}' not found",
                model_name
            )));
        }

        self.source.storage().write_config(&updated_config).await?;
        Ok(())
    }

    pub async fn upsert_integration(
        &self,
        integration: crate::config::model::Integration,
    ) -> Result<(), OxyError> {
        let mut updated_config = self.config_for_write().await;
        let kind = integration_kind(&integration);

        if let Some(slot) = updated_config
            .integrations
            .iter_mut()
            .find(|i| integration_kind(i) == kind)
        {
            *slot = integration;
        } else {
            updated_config.integrations.push(integration);
        }
        self.source.storage().write_config(&updated_config).await?;
        Ok(())
    }

    pub async fn remove_integration_by_kind(&self, kind: &str) -> Result<(), OxyError> {
        let mut updated_config = self.config_for_write().await;
        let initial_len = updated_config.integrations.len();
        updated_config
            .integrations
            .retain(|i| integration_kind(i) != kind);
        if updated_config.integrations.len() == initial_len {
            return Ok(());
        }
        self.source.storage().write_config(&updated_config).await?;
        Ok(())
    }
}

fn integration_kind(integration: &crate::config::model::Integration) -> &'static str {
    use crate::config::model::IntegrationType;
    match &integration.integration_type {
        IntegrationType::Omni(_) => "omni",
        IntegrationType::Looker(_) => "looker",
        IntegrationType::Toast(_) => "toast",
        IntegrationType::ToastAnalytics(_) => "toast_analytics",
        IntegrationType::OpenWeatherMap(_) => "openweathermap",
        IntegrationType::BestTime(_) => "besttime",
        IntegrationType::Unifi(_) => "unifi",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigBuilder;

    /// A manager reading the working copy has no revision, so every boundary
    /// read is a fall-through rather than a query. This is the arm that keeps
    /// local mode and feature branches on the filesystem.
    ///
    /// Ported from `server/api/compiled_read.rs` when that layer was deleted.
    /// Its companion — that dropping the capability leaves the origin alone —
    /// lives in `crates/app/tests/platform/walker_storage_divergence.rs`, which can
    /// build the compiled combination.
    #[tokio::test]
    async fn a_working_copy_manager_reports_no_revision() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
            .await
            .expect("write config");

        let manager = ConfigBuilder::new()
            .with_workspace_path(dir.path())
            .expect("workspace path")
            .build_with_working_copy(Origin::Disk, super::super::OnMissing::Empty)
            .await
            .expect("manager");

        assert_eq!(manager.origin(), Origin::Disk);
        assert_eq!(
            manager.revision_id(),
            None,
            "no revision means every read falls through to the working copy"
        );
    }

    /// The wide version: every read whose disk arm goes through `disk()` is on
    /// a manager the ide hands out as `WorkspaceManagerReadOnly`, and an
    /// UNPROMOTED workspace — or an ide previewing a feature branch — has
    /// `Origin::Disk`, so the compiled arm is not even tried. All three of
    /// these answered `NoSource` for files sitting in the workspace.
    #[tokio::test]
    async fn a_read_only_manager_serves_an_unpromoted_workspace() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::create_dir_all(dir.path().join("agents"))
            .await
            .expect("mkdir");
        tokio::fs::write(
            dir.path().join("agents/sales.agentic.yml"),
            "llm:\n  ref: m\n",
        )
        .await
        .expect("write agent");
        tokio::fs::write(dir.path().join(".monitor.yml"), "monitors: []\n")
            .await
            .expect("write monitor");

        let readonly = disk_manager(dir.path()).await.into_read_only();
        assert_eq!(readonly.origin(), Origin::Disk);

        let agents = readonly.list_analytics_agents().await;
        let monitors = readonly.monitor_config().await;
        let automation = readonly
            .automation_definition("automations/sales.automation.yml")
            .await;

        assert!(agents.is_ok(), "list_analytics_agents: {agents:?}");
        assert!(monitors.is_ok(), "monitor_config: {monitors:?}");
        assert!(automation.is_ok(), "automation_definition: {automation:?}");
    }

    async fn disk_manager(dir: &std::path::Path) -> ConfigManager<WorkingCopy> {
        tokio::fs::write(dir.join("config.yml"), "models: []\ndatabases: []\n")
            .await
            .expect("write config");
        ConfigBuilder::new()
            .with_workspace_path(dir)
            .expect("workspace path")
            .build_with_working_copy(Origin::Disk, super::super::OnMissing::Empty)
            .await
            .expect("manager")
    }

    /// `Origin::Disk` READS the disk. It used to answer `Ok(None)` — "I am not
    /// looking" wearing the costume of "there is nothing there" — which left
    /// every caller to write the working-copy arm itself. Two of them got it
    /// wrong in the same way.
    #[tokio::test]
    async fn a_disk_origin_reads_the_working_copy_not_none() {
        let dir = tempfile::tempdir().expect("tempdir");
        tokio::fs::create_dir_all(dir.path().join("automations"))
            .await
            .expect("mkdir");
        tokio::fs::write(
            dir.path().join("automations/daily.automation.yml"),
            "name: daily\ntasks: []\n",
        )
        .await
        .expect("write automation");

        let manager = disk_manager(dir.path()).await;
        let definition = manager
            .automation_definition("automations/daily.automation.yml")
            .await
            .expect("read")
            .expect("the file is on disk, so this is Some");

        assert_eq!(
            definition.get("name").and_then(|v| v.as_str()),
            Some("daily"),
            "the disk arm must parse the YAML the same way the compiler stores \
             it — `oxy_compile` writes serde_yaml::from_str::<Value> verbatim"
        );
    }

    /// The distinction the whole error type exists for: a file that is not
    /// there is `Ok(None)`, and it must not be confused with the source not
    /// being there. Both used to be `Ok(None)`.
    #[tokio::test]
    async fn a_missing_file_is_none_and_a_missing_source_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = disk_manager(dir.path()).await;

        assert!(
            manager
                .automation_definition("automations/nope.automation.yml")
                .await
                .expect("a missing file is not an error")
                .is_none(),
            "the working copy is here and the file is not — that is a real None"
        );

        let diskless = manager.without_working_copy();
        let err = diskless
            .automation_definition("automations/nope.automation.yml")
            .await
            .expect_err("no compiled revision and no working copy is not a miss");
        assert!(
            err.retryable(),
            "nothing to read from is a platform state the caller should retry, \
             not a 404 that blames the workspace: got {err}"
        );
    }
    /// The third case the pair above does not cover: the file is THERE and we
    /// could not read it.
    ///
    /// `Err(_) => Ok(None)` folded that into "the workspace declares no
    /// `.monitor.yml`", which is a claim about the customer's configuration
    /// made after a failure to look — the same conflation the compiled arm of
    /// `root_singleton` already refuses one branch above.
    ///
    /// A directory in the file's place rather than a chmod: it produces a
    /// non-`NotFound` I/O error on every platform and is not defeated by the
    /// test running as root.
    #[tokio::test]
    async fn an_unreadable_singleton_is_an_error_not_an_absent_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = disk_manager(dir.path()).await;

        assert!(
            manager
                .monitor_config()
                .await
                .expect("no `.monitor.yml` at all is not an error")
                .is_none(),
            "absence is still absence"
        );

        std::fs::create_dir(dir.path().join(".monitor.yml")).expect("mkdir .monitor.yml");

        let err = manager
            .monitor_config()
            .await
            .expect_err("a file that cannot be read is not a workspace that declares none");
        assert!(
            err.retryable(),
            "a disk fault is ours, not the customer's YAML — `list_monitors` \
             turns non-retryable into a 400: got {err}"
        );
    }
    /// A read on a diskless node must not manufacture the workspace root.
    ///
    /// `runtime_state_dir` creates what it resolves and the fallback lives
    /// inside the root, so one `GET /charts/{file}` — a `FleetOk` route a
    /// replica answers — used to leave an empty root behind. That is the exact
    /// shape `state_dir_path`'s doc warns about, and it defeats every other
    /// guard on this type: once the root exists, `disk()`'s `is_dir()` check
    /// passes for that workspace on that node forever.
    ///
    /// The second assertion matters as much as the first. It pins the
    /// CONSEQUENCE — that absence keeps reading as an error rather than as an
    /// empty workspace — which is what someone deleting the guard would
    /// actually be breaking.
    #[tokio::test]
    async fn a_runtime_state_dir_read_does_not_manufacture_the_workspace_root() {
        // SAFETY: nextest runs each test in its own process, so the removal
        // cannot leak. Without it a set `OXY_STATE_DIR` wins in `state_dir_path`
        // and the fallback under test is never reached — the check would pass
        // while inspecting nothing.
        unsafe { std::env::remove_var("OXY_STATE_DIR") };

        let parent = tempfile::tempdir().expect("tempdir");
        let root = parent.path().join("never-cloned");
        assert!(!root.is_dir(), "precondition: this node never cloned it");

        let replica = crate::config::ConfigBuilder::new()
            .with_workspace_path(&root)
            .expect("workspace path")
            .build_with_working_copy(Origin::Disk, crate::config::OnMissing::Empty)
            .await
            .expect("a manager builds from the database column, unstat-ed")
            .into_read_only();

        assert!(
            replica.working_copy().is_some(),
            "the slot is full on a replica — that is the whole hazard"
        );

        let charts = replica.charts_dir();

        assert!(
            !root.exists(),
            "reading a chart path created the workspace root at {}",
            charts.display()
        );
        assert!(
            replica.list_analytics_agents().await.is_err(),
            "with the root manufactured, this answers Ok([]) — the customer \
             configured nothing, from a node that never looked"
        );
    }
    /// A revision-pinned manager on a pod that HAS the file still answers from
    /// the revision, and `Origin::Disk` still reads its directory.
    ///
    /// The helper automations and pipelines use must not fall back to the
    /// working copy on a compiled miss or on a failed compiled read. The file is
    /// on disk in every case, so an implementation that falls back returns
    /// `Some` and the assertions fail.
    #[tokio::test]
    async fn a_revision_pinned_manager_does_not_answer_from_the_files_it_holds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let automations = dir.path().join("automations");
        std::fs::create_dir_all(&automations).expect("mkdir");
        std::fs::write(
            automations.join("on_disk.automation.yml"),
            "name: on_disk\ntasks: []\n",
        )
        .expect("write");
        let path = "automations/on_disk.automation.yml";

        let pinned = crate::config::ConfigBuilder::new()
            .with_workspace_path(dir.path())
            .expect("workspace path")
            .build_with_provided_config_and_working_copy(
                serde_yaml::from_str("models: []\ndatabases: []\n").expect("config"),
                Origin::Compiled {
                    workspace_id: Uuid::new_v4(),
                    revision_id: Uuid::new_v4(),
                },
            )
            .expect("manager");
        assert!(
            pinned.can_read_disk(),
            "precondition: this pod has the files"
        );

        let miss = pinned.revision_definition(async { Ok(None) }, path).await;
        assert!(
            matches!(miss, Ok(None)),
            "the revision does not have it, and the directory is not asked: {miss:?}"
        );
        let down = pinned
            .revision_definition(
                async { Err(ArtifactError::Backend("connection refused".into())) },
                path,
            )
            .await;
        assert!(
            matches!(down, Err(ref e) if e.retryable()),
            "a failed read is a retryable error, not a read of the directory: {down:?}"
        );
        let hit = pinned
            .revision_definition(
                async { Ok(Some(serde_json::json!({ "name": "rev" }))) },
                path,
            )
            .await;
        assert_eq!(
            hit.expect("read").expect("row")["name"],
            "rev",
            "and a row is served as is"
        );

        let disk = disk_manager(dir.path()).await;
        let read = disk
            .revision_definition(async { Ok(None) }, path)
            .await
            .expect("read")
            .expect("the directory this manager was built over");
        assert_eq!(read["name"], "on_disk");
    }

    /// The line every caller labelling these errors depends on.
    ///
    /// A file that is present and unparseable is the workspace's content, and
    /// `retryable()` is `!matches!(self, Config(_))` — so it must come back
    /// NOT retryable, and a source that could not be read must come back
    /// retryable. `resolve_automation_yaml` picks 422 vs 503 off exactly this,
    /// after a version that inferred "unavailable" from the call site instead
    /// and answered `Retry-After` for a YAML typo.
    #[tokio::test]
    async fn a_file_that_will_never_parse_is_not_retryable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let manager = disk_manager(dir.path()).await;
        let automations = dir.path().join("automations");
        std::fs::create_dir_all(&automations).expect("mkdir");
        std::fs::write(
            automations.join("broken.automation.yml"),
            "tasks: [\n  - unclosed\n",
        )
        .expect("write");

        let err = manager
            .automation_definition("automations/broken.automation.yml")
            .await
            .expect_err("a file that does not parse is not a miss");
        assert!(
            !err.retryable(),
            "a typo cannot be retried away; the caller answers 422 off this: {err}"
        );

        // The third shape, and the one the 422 was accidentally built on: the
        // file is THERE and cannot be read. A directory in its place produces a
        // non-`NotFound` I/O error on every platform and is not defeated by
        // running as root. Reporting it as content would tell an operator that
        // a half-mounted volume is the tenant's YAML.
        let unreadable_dir = automations.join("unreadable.automation.yml");
        std::fs::create_dir(&unreadable_dir).expect("mkdir");
        let err = manager
            .automation_definition("automations/unreadable.automation.yml")
            .await
            .expect_err("a file that cannot be read is not a miss");
        assert!(
            err.retryable(),
            "a disk fault is ours, not the customer's YAML — 503, never 422: {err}"
        );

        let unreadable = manager.without_working_copy();
        let err = unreadable
            .automation_definition("automations/broken.automation.yml")
            .await
            .expect_err("no source at all is not a miss either");
        assert!(
            err.retryable(),
            "nothing to read from is the platform's, and the caller answers 503: {err}"
        );
    }
    /// A read-only manager on a node without the files must refuse legibly.
    ///
    /// `workspace_file_resolver` escalated on the slot, and the slot is always
    /// full on a replica — so `try_resolve_file` went on to `resolve_file`,
    /// which fails inside `canonicalize`. The caller got a raw `IOError` naming
    /// a path it never wrote, instead of the sentence the `None` arm already
    /// carries.
    #[tokio::test]
    async fn a_read_only_manager_without_the_files_says_so() {
        let parent = tempfile::tempdir().expect("tempdir");
        let absent = parent.path().join("never-cloned");

        let replica = crate::config::ConfigBuilder::new()
            .with_workspace_path(&absent)
            .expect("workspace path")
            .build_with_working_copy(Origin::Disk, crate::config::OnMissing::Empty)
            .await
            .expect("a manager builds from the database column, unstat-ed")
            .into_read_only();

        assert!(
            replica.working_copy().is_some(),
            "slot full, directory absent — the replica shape"
        );
        assert!(
            replica.workspace_file_resolver().is_none(),
            "a handle is not a filesystem; escalating on one is the bug"
        );

        let err = replica
            .try_resolve_file("agents/analytics.agentic.yml")
            .await
            .expect_err("there is nothing here to resolve against");
        let message = err.to_string();
        assert!(
            message.contains("does not have"),
            "the caller needs the condition, not a canonicalize failure: {message}"
        );
    }

    /// The review finding this guards: the compiled arm
    /// (`oxy_compile::walker`) and the working-copy arm (`storage.rs`) used to
    /// disagree on the skip set — root-prefix-only vs any-depth — so
    /// `build/x.simulation.yml` and `sub/target/x.simulation.yml` existed on
    /// one arm and not the other. Both now call the same
    /// `oxy_compile::walker::is_skipped`; assert they still drop the same
    /// paths rather than trusting the shared call site by inspection.
    #[tokio::test]
    async fn list_simulations_and_the_walker_drop_the_same_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let write = |rel: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "name: w").unwrap();
        };

        write("legit/kept.simulation.yml");
        write("build/dropped_build.simulation.yml");
        write("sub/target/dropped_target.simulation.yml");
        write(".git/dropped_git.simulation.yml");
        write(".oxy_state/dropped_state.simulation.yml");
        write(".draft.simulation.yml");
        write("legit/.wip.simulation.yml");
        // The `.test.` rule scopes to the file NAME, not the whole path — a
        // directory that happens to carry `.test.` (a fixtures folder, say)
        // is not a build dir and must not be skipped like one. Pinned here
        // because this is exactly the divergence the walker fix closed: a
        // whole-path match would drop this file while `list_simulations`
        // kept it, splitting the two arms again.
        write("worlds/q3.test.grid/w.simulation.yml");
        // …but a file NAME carrying `.test.` IS a test fixture mirroring an
        // entity extension, and both arms must drop it. This one ends
        // `.simulation.yml`, so the working copy's extension match claims it
        // while the walker's file-name rule drops it — the exact
        // divergence this test exists to catch, one level down from the
        // directory case above.
        write("worlds/baseline.test.simulation.yml");

        let manager = disk_manager(root).await;
        let working_copy: std::collections::BTreeSet<String> = manager
            .list_simulations()
            .await
            .expect("list_simulations")
            .into_iter()
            .map(|s| s.file_path)
            .collect();

        let compiled: std::collections::BTreeSet<String> = oxy_compile::walker::discover(root)
            .expect("discover")
            .into_iter()
            .filter(|f| matches!(f.kind, oxy_compile::walker::FileKind::Simulation))
            .map(|f| f.rel_path)
            .collect();

        let expected: std::collections::BTreeSet<String> = [
            "legit/kept.simulation.yml".to_string(),
            ".draft.simulation.yml".to_string(),
            "legit/.wip.simulation.yml".to_string(),
            "worlds/q3.test.grid/w.simulation.yml".to_string(),
        ]
        .into_iter()
        .collect();

        assert_eq!(working_copy, expected, "working-copy arm");
        assert_eq!(compiled, expected, "compiled arm (walker)");
        assert_eq!(
            working_copy, compiled,
            "both arms must agree on what a workspace enumerates"
        );
    }

    /// A pruned path must leave a trace on the working-copy arm too.
    ///
    /// The walker already logs its drops at DEBUG. This arm — the one that
    /// backs `list_analytics_agents` / `list_apps` / `list_pipelines` /
    /// `list_simulations` in the IDE — pruned silently, so "my agent
    /// disappeared" had nothing to grep for on the side a developer is
    /// actually looking at. DEBUG rather than WARN for the same reason as the
    /// walker: `node_modules/` is the common case.
    #[tokio::test]
    async fn a_pruned_directory_and_a_test_fixture_are_reported_at_debug() {
        use std::io::Write;
        use std::sync::{Arc, Mutex};

        #[derive(Clone, Default)]
        struct SharedBuf(Arc<Mutex<Vec<u8>>>);
        impl Write for SharedBuf {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl tracing_subscriber::fmt::MakeWriter<'_> for SharedBuf {
            type Writer = Self;
            fn make_writer(&self) -> Self::Writer {
                self.clone()
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let write = |rel: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "name: a").unwrap();
        };
        write("agents/kept.agentic.yml");
        write("sub/build/stray.agentic.yml");
        write("agents/kept.test.agentic.yml");

        let manager = disk_manager(root).await;

        let buf = SharedBuf::default();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(buf.clone())
            .with_ansi(false)
            .finish();
        // `with_default` is thread-local, but the listing is async and may
        // resume on another worker, so drive it to completion inside the
        // guard on this thread rather than awaiting across it.
        let agents = tracing::subscriber::with_default(subscriber, || {
            futures::executor::block_on(manager.list_analytics_agents())
        })
        .expect("list_analytics_agents");
        let logs = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();

        assert_eq!(
            agents.into_iter().map(|a| a.file_path).collect::<Vec<_>>(),
            vec!["agents/kept.agentic.yml".to_string()],
            "one agent survives"
        );
        assert!(
            logs.contains("pruning a skipped directory") && logs.contains("build"),
            "a directory pruned by the skip set must be greppable; got:\n{logs}"
        );
        assert!(
            logs.contains("kept.test.agentic.yml"),
            "a file dropped as a test fixture must be greppable; got:\n{logs}"
        );
    }

    /// `list_analytics_agents` claims both arms agree on the test-mirror
    /// convention; assert it rather than trusting the claim.
    ///
    /// Two spellings, and only one of them used to be safe by accident. The
    /// conventional `sales.agent.test.yml` ends `.test.yml`, so no
    /// `.agentic.yml` extension match ever claimed it on either arm.
    /// `sales.test.agentic.yml` DOES end `.agentic.yml` — the working copy
    /// listed it, the walker dropped it, and the IDE resolved an agent the
    /// fleet 404'd. The shared `.test.` file-name rule is what closes that.
    #[tokio::test]
    async fn agents_agree_on_the_test_mirror_convention() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let write = |rel: &str| {
            let path = root.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "name: a").unwrap();
        };

        write("agents/sales.agentic.yml");
        // The convention `load_test_config` infers a target from.
        write("agents/sales.agent.test.yml");
        // The spelling that ends in a real entity extension.
        write("agents/sales.test.agentic.yml");
        // A fixtures DIRECTORY is not a build dir: the agent under it stays.
        write("agents/v1.test.cases/nested.agentic.yml");

        let manager = disk_manager(root).await;
        let working_copy: std::collections::BTreeSet<String> = manager
            .list_analytics_agents()
            .await
            .expect("list_analytics_agents")
            .into_iter()
            .map(|a| a.file_path)
            .collect();

        let compiled: std::collections::BTreeSet<String> = oxy_compile::walker::discover(root)
            .expect("discover")
            .into_iter()
            .filter(|f| matches!(f.kind, oxy_compile::walker::FileKind::AgenticAgent))
            .map(|f| f.rel_path)
            .collect();

        let expected: std::collections::BTreeSet<String> = [
            "agents/sales.agentic.yml".to_string(),
            "agents/v1.test.cases/nested.agentic.yml".to_string(),
        ]
        .into_iter()
        .collect();

        assert_eq!(working_copy, expected, "working-copy arm");
        assert_eq!(compiled, expected, "compiled arm (walker)");
        assert_eq!(
            working_copy, compiled,
            "both arms must agree on what a workspace enumerates"
        );
    }
}
