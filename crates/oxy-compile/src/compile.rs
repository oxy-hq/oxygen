//! The compile orchestrator.
//!
//! `compile_workspace` is the single entry point used by the CLI, by
//! the future TaskSpec wrapper, and by tests. It:
//!
//!   1. Discovers every interesting file (delegated to `walker`).
//!   2. Parses each into a generic JSON value (no strict typing in
//!      Phase 1.6a — that lands when runtime starts *reading* the
//!      rows in Phase 1.6c).
//!   3. Builds an in-memory list of `CompiledRow` values and a
//!      parallel list of per-file failures.
//!   4. Hands them to `writer` which materialises them into Postgres
//!      under a single revision_id.
//!
//! Phase 1.6a deliberately does NOT promote the new revision to
//! `workspaces.current_revision_id` (observation mode). The runtime
//! still reads YAML from disk; we're only verifying that compile
//! produces sane rows in production.

use crate::errors::CompileError;
use crate::outcome::{CompileOutcome, FailureKind, FileFailure, Promotion, RevisionStatus};
use crate::walker::{AutomationKind, DiscoveredFile, FileKind, discover};
use crate::writer::{
    FinaliseInput, FinaliseOutcome, RevisionContext, finalise_revision, insert_compiling_revision,
    mark_failed,
};
use entity::workspace_compiled_configs::CompiledConfig;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::Path;
use tracing::{debug, info, instrument, warn};
use uuid::Uuid;

/// Which generation of the compiled row set a revision carries, stamped on
/// `revisions.schema_version`.
///
/// Bump it when a compile starts writing a kind that a reader must be able to
/// tell from "this revision was never asked". An empty table cannot say which:
///
/// * `1` — everything up to `schema_migration_definitions`.
/// * `2` — plus `context_document_definitions`
///   ([`crate::context_documents::SINCE_SCHEMA_VERSION`]).
///
/// Reuse is keyed on it too (`lookup_idempotent_revision`,
/// `find_reusable_revision`), so a bump stops an older revision of the same
/// SHA being handed back as if it carried the new kind.
pub const CURRENT_SCHEMA_VERSION: i32 = 2;

/// How recent a successful revision has to be for an idempotent
/// re-compile to reuse it. Hardcoded to one hour; a same-SHA re-compile
/// inside the window short-circuits and returns the existing revision.
/// The window is intentionally not env-tunable — the partial unique
/// index in Postgres is the multi-worker correctness primitive; this
/// window is just a single-worker fast path that does not need a knob.
const IDEMPOTENCY_WINDOW_SECS: i64 = 60 * 60;

/// One row queued for insertion. Mirrors the per-entity tables.
#[derive(Debug, Clone)]
pub enum CompiledRow {
    Config(CompiledConfig),
    Agent(CompiledAgent),
    View(CompiledView),
    Topic(CompiledTopic),
    App(CompiledApp),
    Automation(CompiledAutomation),
    VerifiedQuery(CompiledVerifiedQuery),
    SchemaMigration(CompiledSchemaMigration),
    Pipeline(CompiledPipeline),
    Reference(CompiledReference),
    MonitorConfig(CompiledMonitorConfig),
    ReconcileConfig(CompiledReconcileConfig),
    WorldModelConfig(CompiledWorldModelConfig),
    Simulation(CompiledSimulation),
    ContextDocument(CompiledContextDocument),
}

/// One `.md` an agent's `context:` reaches. The body IS the artifact, as it is
/// for a verified query: there is nothing to parse.
#[derive(Debug, Clone)]
pub struct CompiledContextDocument {
    pub file_path: String,
    pub content_sha256: String,
    pub content: String,
}

/// One `.simulation.yml`.
///
/// The body is carried as an opaque `Value`, like every other kind here —
/// nothing in `oxy-compile` deserializes a domain type, and a `SimulationSpec`
/// that parses but declares an incoherent world (an unreachable optimum, too
/// little history to fit on) is a *run* failure with a diagnosable message, not
/// a compile failure that takes the whole revision down with it. Compile still
/// catches what it is the authority on: the file is well-formed YAML.
#[derive(Debug, Clone)]
pub struct CompiledSimulation {
    pub name: String,
    pub file_path: String,
    pub definition: Value,
}

#[derive(Debug, Clone)]
pub struct CompiledMonitorConfig {
    pub definition: Value,
}

#[derive(Debug, Clone)]
pub struct CompiledReconcileConfig {
    pub definition: Value,
}

#[derive(Debug, Clone)]
pub struct CompiledWorldModelConfig {
    pub definition: Value,
}

#[derive(Debug, Clone)]
pub struct CompiledAgent {
    pub name: String,
    pub file_path: String,
    pub definition: Value,
}

#[derive(Debug, Clone)]
pub struct CompiledView {
    pub name: String,
    pub file_path: String,
    pub definition: Value,
}

#[derive(Debug, Clone)]
pub struct CompiledTopic {
    pub name: String,
    pub file_path: String,
    pub definition: Value,
}

#[derive(Debug, Clone)]
pub struct CompiledApp {
    pub file_path: String,
    pub name: String,
    pub definition: Value,
    pub published: bool,
}

#[derive(Debug, Clone)]
pub struct CompiledAutomation {
    pub file_path: String,
    pub name: String,
    pub extension: String,
    pub definition: Value,
}

#[derive(Debug, Clone)]
pub struct CompiledVerifiedQuery {
    pub file_path: String,
    pub content_sha256: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct CompiledPipeline {
    pub name: String,
    pub file_path: String,
    pub definition: Value,
}

#[derive(Debug, Clone)]
pub struct CompiledReference {
    pub from_kind: String,
    pub from_name: String,
    pub to_kind: String,
    pub to_name: String,
}

/// Veto hook for a compiled `config.yml` that would not deserialise at read
/// time. Implemented by `oxy-app` (which owns the strict `Config` type — this
/// crate deliberately has no platform deps); `None` for callers/tests that
/// don't need the gate.
///
/// The point is to convert the entire "compiled config won't deserialise"
/// failure class from a runtime fleet-wide 503 into a compile-time failure:
/// when `check` returns `Err`, the config becomes a `FailureKind::Validation`
/// failure, the revision is marked `Failed`, and it is never promoted — so the
/// previous good revision keeps serving. It backstops ANY compile transform
/// (today: secret redaction + the DuckDB→S3 mirror), not just one known bug.
/// See oxygen-internal#2520 (the `s3_secret_type` outage this prevents).
pub trait ConfigGate: Send + Sync {
    /// `Ok(())` to accept, `Err(message)` to reject (operator-facing reason).
    fn check(&self, cfg: &CompiledConfig) -> Result<(), String>;
}

/// Public inputs to a compile run.
pub struct CompileRequest<'a> {
    pub db: &'a DatabaseConnection,
    pub workspace_id: Uuid,
    pub workspace_path: &'a Path,
    /// The SHA the operator should see on this revision. If None, a unique
    /// `local-<uuid>` is minted (NOT a constant) so repeated working-copy
    /// compiles don't collide on the idempotency unique index — useful for
    /// `oxy compile` runs against a working copy with uncommitted edits.
    pub git_sha: Option<String>,
    pub branch: Option<String>,
    /// Identifies the binary version that produced this revision.
    pub compiler_version: String,
    /// When true AND the compile succeeds AND `kind == "main"`,
    /// atomically updates `workspaces.current_revision_id` to the
    /// new revision inside the finalise transaction. Defaults to
    /// false: writing rows is safe, changing what the runtime reads
    /// is the decision, so it has to be asked for.
    pub promote: bool,
    /// `main` (default) or `draft`. Drafts are scoped to a single
    /// `owner_user_id` and are never promoted to current_revision_id
    /// even when `promote` is true.
    pub kind: RevisionKind,
    /// Required when `kind == Draft`; ignored otherwise.
    pub owner_user_id: Option<Uuid>,
    /// Optional read-time-deserialisation gate (see [`ConfigGate`]). Supplied
    /// by oxy-app on the production compile paths (worker + CLI); `None` in
    /// tests and callers that don't have the strict `Config` type to hand.
    pub config_gate: Option<std::sync::Arc<dyn ConfigGate>>,
}

/// `main` vs `draft` vs `staging` revision kinds. Strictly typed so a caller
/// can't accidentally pass an unrecognised kind string into the writer.
///
/// Only `Main` is ever promoted. `Staging` is a branch head compiled for a
/// custom-app draft build to pin (`app_builds.semantic_revision_id`); it is
/// shared by every admin who previews that build, so unlike `Draft` it has no
/// owner. See `internal-docs/compile-boundary.md` § "Staging revisions".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RevisionKind {
    #[default]
    Main,
    Draft,
    Staging,
}

impl RevisionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            RevisionKind::Main => "main",
            RevisionKind::Draft => "draft",
            RevisionKind::Staging => "staging",
        }
    }

    /// Parse a stored `revisions.kind` value. `None` for anything unknown.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "main" => Some(RevisionKind::Main),
            "draft" => Some(RevisionKind::Draft),
            "staging" => Some(RevisionKind::Staging),
            _ => None,
        }
    }

    /// True only for `Main`: the one kind `workspaces.current_revision_id`
    /// may point at. Drafts and staging revisions are never promoted, even
    /// with `promote: true`.
    pub fn is_promotable(&self) -> bool {
        matches!(self, RevisionKind::Main)
    }

    /// The stored kinds an idempotent re-compile of this kind may reuse.
    /// A staging compile reuses a ready main revision of the same SHA too —
    /// a pin reads rows by revision id, so the kind is irrelevant to it —
    /// but a main compile never reuses a staging row, or its promote would
    /// move `current_revision_id` onto one.
    pub fn reusable_kinds(&self) -> &'static [&'static str] {
        match self {
            RevisionKind::Main => &["main"],
            RevisionKind::Draft => &[],
            RevisionKind::Staging => &["staging", "main"],
        }
    }
}

/// The end-to-end compile entry point.
#[instrument(
    name = "oxy_compile",
    skip_all,
    fields(workspace_id = %request.workspace_id, git_sha = %request.git_sha.as_deref().unwrap_or("local")),
)]
pub async fn compile_workspace(
    request: CompileRequest<'_>,
) -> Result<CompileOutcome, CompileError> {
    // A compile with no addressable SHA (CLI `oxy compile` on a working tree,
    // the lazy self-heal) is "local". Mint a UNIQUE `local-<uuid>` rather than
    // the constant "local": the partial unique index
    // `idx_revisions_idempotent_ready_main` is keyed on (workspace_id,
    // git_sha), so two distinct working-tree compiles that both recorded the
    // constant "local" would collide — the second finalise hits the unique
    // violation, returns SupersededBy the FIRST, and silently discards the
    // newer compile (returning stale data). A unique sha makes each local
    // compile its own revision; `promote_revision`'s started_at causality
    // clause then promotes the newest. Real-SHA compiles are unaffected and
    // keep their (correct) idempotent dedup. See oxygen-internal#2520.
    let is_local = request.git_sha.is_none();
    let git_sha = request
        .git_sha
        .unwrap_or_else(|| format!("local-{}", Uuid::new_v4()));
    let kind_str = request.kind.as_str();
    info!(
        workspace_path = %request.workspace_path.display(),
        promote = request.promote,
        kind = kind_str,
        "compile started"
    );

    // Drafts MUST carry an owner_user_id so the future user-scoped
    // read path knows whom to serve them to. Catch the misconfigured
    // call here rather than letting it land as an integrity check
    // surprise downstream.
    if matches!(request.kind, RevisionKind::Draft) && request.owner_user_id.is_none() {
        return Err(CompileError::Internal(
            "draft revision requires owner_user_id".to_string(),
        ));
    }

    // Idempotency short-circuit. Skip the heavy work when a recent
    // successful revision already exists for this (workspace_id,
    // git_sha, compiler_version, schema_version) tuple and the request
    // isn't trying to overwrite via
    // a different kind/owner_user_id. Two callers want this:
    //
    //   - Operator-triggered re-runs against an unchanged SHA from
    //     the admin "Run compile now" form, or repeated IDE Compile
    //     button clicks on the same HEAD.
    //   - Multiple workspaces inheriting the same commit (e.g. forks)
    //     that get triggered close together.
    //
    // Local compiles (no addressable SHA — `is_local`, minted as a unique
    // `local-<uuid>`) opt out: local edits aren't addressable by SHA, so
    // identity is the working tree itself, which can change between
    // invocations. The unique sha also keeps them off the idempotency unique
    // index so two distinct working-tree compiles don't supersede each other.
    //
    // When `promote` is requested AND the existing revision isn't
    // already current, we still execute the lightweight promotion
    // path so `workspaces.current_revision_id` ends up pointing at
    // the matching revision. That keeps "run compile now with
    // promote" semantically correct even when the SHA is unchanged.
    if !is_local
        && !matches!(request.kind, RevisionKind::Draft)
        && let Some(existing) = lookup_idempotent_revision(
            request.db,
            request.workspace_id,
            request.kind,
            &git_sha,
            &request.compiler_version,
            CURRENT_SCHEMA_VERSION,
        )
        .await?
    {
        info!(
            workspace_id = %request.workspace_id,
            git_sha = %git_sha,
            revision_id = %existing.revision_id,
            "compile idempotent — reusing existing successful revision"
        );
        if request.promote && request.kind.is_promotable() {
            // A revision carrying DDL is never promoted from here. Its
            // tables may not exist in the tenant database, and on this
            // path nobody has applied them — the compile that first
            // produced this revision either deferred for the same reason
            // or ran with `promote: false`. Hand the caller `Deferred`
            // and let apply-then-promote run. `apply_to_org` is
            // idempotent, so an already-applied set costs one ledger
            // read rather than a re-run of the DDL.
            match stored_schema_migration_count(request.db, existing.revision_id).await? {
                0 => match crate::writer::promote_existing(
                    request.db,
                    request.workspace_id,
                    existing.revision_id,
                )
                .await
                {
                    Ok(promotion) => {
                        return Ok(existing.into_outcome(git_sha, request.branch, promotion));
                    }
                    Err(e) => {
                        warn!(
                            ?e,
                            "idempotent promote failed; falling through to full compile"
                        );
                    }
                },
                n => {
                    return Ok(existing.into_outcome(
                        git_sha,
                        request.branch,
                        Promotion::Deferred {
                            schema_migration_count: n,
                        },
                    ));
                }
            }
        } else {
            return Ok(existing.into_outcome(git_sha, request.branch, Promotion::NotRequested));
        }
    }

    let ctx = insert_compiling_revision(
        request.db,
        request.workspace_id,
        &git_sha,
        request.branch.as_deref(),
        CURRENT_SCHEMA_VERSION,
        &request.compiler_version,
        kind_str,
        request.owner_user_id,
    )
    .await?;

    // From here on, every error path should mark the revision row as
    // failed so the operator can see it. Use a guard struct so panics
    // / early returns still record.
    let outcome = drive_compile(
        request.db,
        &ctx,
        request.workspace_path,
        &git_sha,
        request.branch,
        request.promote,
        kind_str,
        request.config_gate.as_deref(),
    )
    .await;

    match outcome {
        Ok(o) => Ok(o),
        Err(e) => {
            warn!(
                ?e,
                "compile failed before finalise; marking revision failed"
            );
            mark_failed(request.db, ctx.revision_id, &e.to_string()).await;
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn drive_compile(
    db: &DatabaseConnection,
    ctx: &RevisionContext,
    workspace_path: &Path,
    git_sha: &str,
    branch: Option<String>,
    promote: bool,
    kind: &str,
    config_gate: Option<&dyn ConfigGate>,
) -> Result<CompileOutcome, CompileError> {
    let files = discover(workspace_path)?;
    debug!(file_count = files.len(), "files discovered");

    let mut rows: Vec<CompiledRow> = Vec::new();
    let mut failures: Vec<FileFailure> = Vec::new();
    let mut compiled = 0u32;

    // Track (kind, identifier) so we can flag duplicates within one
    // revision. Two .agentic.yml files with the same `name` would
    // collide on the (revision_id, name) PK and crash the txn;
    // catching it here turns it into a structured per-file failure.
    let mut seen_keys: HashSet<(FileKind, String)> = HashSet::new();

    for file in &files {
        match compile_one(file).await {
            Ok(produced) => {
                for row in produced {
                    if let Some(key) = row_dedupe_key(&row, &file.kind)
                        && !seen_keys.insert((file.kind, key.clone()))
                    {
                        failures.push(FileFailure {
                            path: file.rel_path.clone(),
                            kind: FailureKind::Duplicate,
                            message: format!("duplicate identifier '{}' for this entity kind", key),
                        });
                        continue;
                    }
                    rows.push(row);
                }
                compiled += 1;
            }
            Err(f) => failures.push(f),
        }
    }

    // Mirror local-file DuckDB warehouses to S3 so the stateless fleet can
    // query them (no-op without OXY_COMPILE_BLOB_S3_BUCKET). Only on an
    // otherwise-clean revision — a Failed revision won't be promoted, so
    // there's nothing to serve and no point uploading its data.
    if failures.is_empty()
        && let Some(CompiledRow::Config(cfg)) = rows
            .iter_mut()
            .find(|r| matches!(r, CompiledRow::Config(_)))
    {
        crate::duckdb_mirror::mirror_duckdb_databases(
            workspace_path,
            ctx.workspace_id,
            &mut cfg.databases,
        )
        .await;
    }

    // Round-trip gate. Runs AFTER every config transform (redaction + the
    // DuckDB→S3 mirror) so we validate exactly what will be served. A compiled
    // config that won't deserialise into the runtime `Config` must never be
    // promoted: that would 503 the whole stateless fleet for this workspace
    // (oxygen-internal#2520). Failing the compile here keeps the previous good
    // revision serving and surfaces a clear `[invalid]` failure to the
    // operator, instead of a silent runtime outage.
    if let Some(gate) = config_gate
        && let Some(CompiledRow::Config(cfg)) =
            rows.iter().find(|r| matches!(r, CompiledRow::Config(_)))
        && let Err(message) = gate.check(cfg)
    {
        let config_path = files
            .iter()
            .find(|f| matches!(f.kind, FileKind::Config))
            .map(|f| f.rel_path.clone())
            .unwrap_or_else(|| "config.yml".to_string());
        warn!(path = %config_path, %message, "compile: config failed the read-time deserialise gate");
        failures.push(FileFailure {
            path: config_path,
            kind: FailureKind::Validation,
            message,
        });
    }

    let status = if failures.is_empty() {
        RevisionStatus::Ready
    } else {
        RevisionStatus::Failed
    };

    let outcome = finalise_revision(
        db,
        ctx,
        FinaliseInput {
            status: status.clone(),
            file_count_seen: files.len() as u32,
            file_count_compiled: compiled,
            file_count_failed: failures.len() as u32,
            failures: &failures,
            rows: &rows,
            promote,
            kind: kind.to_string(),
            schema_migration_count: count_schema_migrations(&rows),
        },
    )
    .await?;

    // Multi-worker idempotency: when the partial unique index
    // rejected our finalise, return the winner's outcome instead.
    let (finished_at, promotion) = match outcome {
        FinaliseOutcome::Committed {
            finished_at,
            promotion,
        } => (finished_at, promotion),
        FinaliseOutcome::SupersededBy {
            revision_id,
            promotion: winner_promotion,
        } => {
            info!(
                losing = %ctx.revision_id,
                winning = %revision_id,
                "compile superseded by concurrent worker — returning winner's outcome"
            );
            // Fetch by PK, NOT through `lookup_idempotent_revision` —
            // that helper is window-gated (won't see a winner that
            // finished outside the idempotency window). The partial
            // unique index that produced this `SupersededBy` outcome
            // is window-independent. The winner's revision_id is
            // already known; load it directly.
            // `winner_promotion` is what the writer actually did to
            // `current_revision_id` on the way out, and it names the winner's
            // revision, not ours. `Deferred` there means both workers saw DDL
            // and neither promoted — this caller drives apply-then-promote,
            // and the apply is idempotent, so both racing it is safe.
            return lookup_revision_by_id(db, revision_id)
                .await?
                .map(|r| r.into_outcome(git_sha.to_string(), branch.clone(), winner_promotion))
                .ok_or_else(|| {
                    CompileError::Internal(format!(
                        "superseded by revision {revision_id} but the row could not be loaded"
                    ))
                });
        }
    };

    info!(
        revision_id = %ctx.revision_id,
        files_seen = files.len(),
        files_compiled = compiled,
        files_failed = failures.len(),
        status = ?status,
        "compile finished"
    );

    Ok(CompileOutcome {
        revision_id: ctx.revision_id,
        status,
        git_sha: git_sha.to_string(),
        branch,
        started_at: ctx.started_at,
        finished_at,
        file_count_seen: files.len() as u32,
        file_count_compiled: compiled,
        file_count_failed: failures.len() as u32,
        failures,
        promotion,
    })
}

/// Count the `schemas/*.sql` rows this compile produced.
///
/// Deliberately counts `CompiledRow`s rather than `DiscoveredFile`s: a DDL
/// file that failed to compile produces no row, and a revision whose DDL
/// did not compile is `Failed` and never promoted anyway. Counting the
/// files instead would defer a promotion that has nothing to apply.
fn count_schema_migrations(rows: &[CompiledRow]) -> u32 {
    rows.iter()
        .filter(|r| matches!(r, CompiledRow::SchemaMigration(_)))
        .count() as u32
}

/// How many `schemas/*.sql` rows an *existing* revision carries.
///
/// The idempotency short-circuit never walks the workspace, so it has no
/// `CompiledRow`s to count — the answer has to come back out of Postgres.
async fn stored_schema_migration_count(
    db: &DatabaseConnection,
    revision_id: Uuid,
) -> Result<u32, CompileError> {
    let n = entity::schema_migration_definitions::Entity::find()
        .filter(entity::schema_migration_definitions::Column::RevisionId.eq(revision_id))
        .count(db)
        .await?;
    Ok(n as u32)
}

/// Per-file compile. Returns one or more rows on success, a structured
/// failure on parse / shape errors.
async fn compile_one(file: &DiscoveredFile) -> Result<Vec<CompiledRow>, FileFailure> {
    let bytes = tokio::fs::read(&file.abs_path)
        .await
        .map_err(|e| FileFailure {
            path: file.rel_path.clone(),
            kind: FailureKind::Io,
            message: e.to_string(),
        })?;
    let content = String::from_utf8_lossy(&bytes).into_owned();

    match file.kind {
        FileKind::Config => compile_config(file, &content),
        FileKind::AgenticAgent => compile_named_yaml(file, &content, |name, file_path, def| {
            CompiledRow::Agent(CompiledAgent {
                name,
                file_path,
                definition: def,
            })
        }),
        FileKind::SemanticView => compile_named_yaml(file, &content, |name, file_path, def| {
            CompiledRow::View(CompiledView {
                name,
                file_path,
                definition: def,
            })
        }),
        FileKind::SemanticTopic => compile_named_yaml(file, &content, |name, file_path, def| {
            CompiledRow::Topic(CompiledTopic {
                name,
                file_path,
                definition: def,
            })
        }),
        FileKind::App => compile_app(file, &content),
        FileKind::Automation(p) => compile_automation(file, &content, p),
        FileKind::AirwayPipeline => compile_named_yaml(file, &content, |name, file_path, def| {
            CompiledRow::Pipeline(CompiledPipeline {
                name,
                file_path,
                definition: def,
            })
        }),
        FileKind::Simulation => compile_named_yaml(file, &content, |name, file_path, def| {
            CompiledRow::Simulation(CompiledSimulation {
                name,
                file_path,
                definition: def,
            })
        }),
        FileKind::VerifiedQuery => compile_verified_query(file, &content),
        FileKind::ContextDocument => compile_context_document(file, &content),
        FileKind::SchemaMigration => compile_schema_migration(file, &content),
        FileKind::MonitorConfig => compile_monitor_config(file, &content),
        FileKind::ReconcileConfig => compile_reconcile_config(file, &content),
        FileKind::WorldModelConfig => compile_world_model_config(file, &content),
    }
}

fn compile_monitor_config(
    file: &DiscoveredFile,
    content: &str,
) -> Result<Vec<CompiledRow>, FileFailure> {
    let value = parse_yaml(file, content)?;
    Ok(vec![CompiledRow::MonitorConfig(CompiledMonitorConfig {
        definition: value,
    })])
}

fn compile_reconcile_config(
    file: &DiscoveredFile,
    content: &str,
) -> Result<Vec<CompiledRow>, FileFailure> {
    let value = parse_yaml(file, content)?;
    Ok(vec![CompiledRow::ReconcileConfig(
        CompiledReconcileConfig { definition: value },
    )])
}

fn compile_world_model_config(
    file: &DiscoveredFile,
    content: &str,
) -> Result<Vec<CompiledRow>, FileFailure> {
    let value = parse_yaml(file, content)?;
    Ok(vec![CompiledRow::WorldModelConfig(
        CompiledWorldModelConfig { definition: value },
    )])
}

fn compile_config(file: &DiscoveredFile, content: &str) -> Result<Vec<CompiledRow>, FileFailure> {
    let value = parse_yaml(file, content)?;
    let cfg =
        build_compiled_config(value, Some(&file.rel_path)).map_err(|message| FileFailure {
            path: file.rel_path.clone(),
            kind: FailureKind::Shape,
            message,
        })?;
    Ok(vec![CompiledRow::Config(cfg)])
}

/// Pure config compilation: split the top-level `config.yml` mapping into the
/// per-column [`CompiledConfig`] shape and strip inline secret literals. No FS,
/// no DB — so it's directly unit-testable and is the single seam the round-trip
/// gate ([`ConfigGate`]) and the round-trip property test exercise. `rel_path`
/// only labels the redaction warning. `Err` carries an operator-facing message.
pub fn build_compiled_config(
    value: Value,
    rel_path: Option<&str>,
) -> Result<CompiledConfig, String> {
    let Value::Object(mut map) = value else {
        return Err("config.yml must be a YAML mapping at the top level".into());
    };

    // Databases is the only field we require to exist; the rest are optional.
    // Empty array is valid — a workspace with no databases yet is a real state
    // during onboarding.
    let mut cfg = CompiledConfig {
        databases: map.remove("databases").unwrap_or(Value::Array(vec![])),
        models: map.remove("models"),
        integrations: map.remove("integrations"),
        repositories: map.remove("repositories"),
        builder_agent: map.remove("builder_agent"),
        mcp: map.remove("mcp"),
        other: None,
    };
    cfg.other = if map.is_empty() {
        None
    } else {
        Some(Value::Object(map))
    };

    // Strip inline secret literals before they hit Postgres. The compiled
    // config lands in `workspace_compiled_configs` — a central, queryable,
    // multi-tenant, per-revision-retained table — so a plaintext `password:` in
    // config.yml would be a worse resting place than the old per-worker FS read.
    // `*_var` references and bare env-var-style references (UPPER_SNAKE values
    // typed as `ManagedSecret`, e.g. ducklake `secret: AWS_S3_SECRET`) are
    // preserved — they NAME a secret in the encrypted store, they aren't the
    // secret, and nulling them would corrupt a structurally-required field. The
    // runtime resolves them from the encrypted secret store at query time.
    //
    // This redaction is best-effort; the round-trip gate (see `drive_compile`)
    // is the actual safety net — it guarantees nothing redaction does can ship a
    // config the fleet can't read.
    let mut redacted = redact_inline_secrets(&mut cfg.databases);
    // EVERY config section that can hold an inline literal must be redacted.
    // `models` carries LLM `api_key:` (e.g. an inline Ollama/OpenAI key);
    // `repositories` carries `git_url:` which can embed credentials inline
    // (https://user:token@host). Keep this list exhaustive over the
    // `map.remove(...)` calls above. See oxygen-internal#2520.
    for field in [
        &mut cfg.models,
        &mut cfg.integrations,
        &mut cfg.repositories,
        &mut cfg.mcp,
        &mut cfg.builder_agent,
        &mut cfg.other,
    ]
    .into_iter()
    .flatten()
    {
        redacted += redact_inline_secrets(field);
    }
    if redacted > 0 {
        tracing::warn!(
            path = ?rel_path,
            redacted,
            "compile: redacted inline secret literal(s) from config.yml — move them to \
             the encrypted secret store via `*_var` references; inline secrets are not \
             carried into the compiled config served to the runtime"
        );
    }
    Ok(cfg)
}

/// Value written over a stripped inline secret literal: the EMPTY STRING, never
/// `null`. Two properties matter, and only `""` has both:
///   1. It deserialises into a REQUIRED `String`/`PathBuf` field (Ollama
///      `api_key`, BigQuery `key_path`). `null` does not — it fails the
///      round-trip gate, which sinks the ENTIRE compile and leaves the
///      workspace unservable on the stateless fleet (oxygen-internal#2528).
///   2. `SecretsManager::resolve_config_value` treats an empty inline value as
///      ABSENT (`!value.is_empty()`), so a field that ALSO carries a `*_var`
///      reference still resolves from the encrypted store. A NON-EMPTY
///      placeholder would break that — the runtime would hand the placeholder
///      back as the credential instead of resolving the var.
const REDACTED_VALUE: &str = "";

/// Recursively replace inline secret *literals* with [`REDACTED_VALUE`] in a
/// compiled config sub-tree. Returns the number of fields redacted. `*_var`
/// references are preserved (they're env-var names, not secrets); only
/// non-empty string values under a sensitive key are stripped.
fn redact_inline_secrets(value: &mut Value) -> usize {
    match value {
        Value::Object(map) => {
            let mut n = 0;
            for (k, v) in map.iter_mut() {
                if is_sensitive_key(k)
                    && matches!(v, Value::String(s) if !s.is_empty() && !looks_like_env_var_ref(s))
                {
                    *v = Value::String(REDACTED_VALUE.to_string());
                    n += 1;
                } else if let Value::String(s) = v {
                    // Non-sensitive key but the value may be a URL with
                    // embedded credentials (e.g. `git_url:
                    // https://user:token@host`, or a connection URL). Strip
                    // the userinfo, keep the URL usable.
                    if let Some(cleaned) = strip_url_credentials(s) {
                        *s = cleaned;
                        n += 1;
                    }
                } else {
                    n += redact_inline_secrets(v);
                }
            }
            n
        }
        Value::Array(arr) => arr.iter_mut().map(redact_inline_secrets).sum(),
        _ => 0,
    }
}

/// A bare environment-variable-style reference (`AWS_S3_SECRET`,
/// `DUCKLAKE_S3_SECRET`): UPPER_SNAKE, no lowercase, no spaces/punctuation.
/// Fields typed as `ManagedSecret` (e.g. ducklake `secret:` / `catalog_path:`)
/// hold one of these — the value NAMES a secret in the encrypted store, it is
/// NOT itself a secret, and it's structurally required. Nulling it would corrupt
/// the config (the `s3_secret_type: config` variant fails to deserialise without
/// `secret`), so redaction must leave it intact. A real inline secret
/// (`hunter2`, `sk-…`, `ghp_…`) is mixed-case / punctuated and won't match, so
/// it is still redacted. The round-trip gate backstops any miss either way.
fn looks_like_env_var_ref(s: &str) -> bool {
    match s.chars().next() {
        Some(c) if c.is_ascii_uppercase() || c == '_' => {}
        _ => return false,
    }
    // Require at least one underscore — env-var word structure. This narrows the
    // preserve set to genuine UPPER_SNAKE names and excludes high-entropy
    // uppercase blobs that are real inline secrets, not references: base32 TOTP
    // seeds (JBSWY3DPEHPK3PXP) and uppercase-hex tokens have no underscores, so
    // they are still redacted. Over-preservation is a SILENT leak (the round-trip
    // gate does NOT catch it), so we bias toward redacting; over-redaction is
    // caught loudly by the gate. The durable fix is entity-aware redaction
    // (knowing a field is `ManagedSecret`-typed) instead of value-shape guessing
    // — tracked as a follow-up (#2524 review).
    s.contains('_')
        && s.chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Strip `user[:pass]@` userinfo from a URL-like string so an inline
/// credential embedded in a `git_url` / connection URL can't survive into
/// the queryable compiled config. Returns `Some(cleaned)` only when
/// userinfo was present and removed; `None` for plain strings / URLs with
/// no credentials, so callers can cheaply detect "was anything redacted".
fn strip_url_credentials(s: &str) -> Option<String> {
    let scheme_end = s.find("://")?;
    let after = scheme_end + 3;
    // Authority runs from after `://` to the next `/` (or end of string).
    let authority_end = s[after..].find('/').map(|i| after + i).unwrap_or(s.len());
    let authority = &s[after..authority_end];
    // userinfo is everything before the LAST `@` in the authority.
    let at = authority.rfind('@')?;
    Some(format!(
        "{}{}{}",
        &s[..after],
        &authority[at + 1..],
        &s[authority_end..]
    ))
}

/// Whether a config key names a secret. Conservative substring match over the
/// credential field names used across the warehouse/integration configs, minus
/// the `*_var` reference suffix (those are kept so the runtime can resolve them
/// from the encrypted secret store).
fn is_sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    if k.ends_with("_var") {
        return false;
    }
    // Type-discriminator keys (serde internal tags like `s3_secret_type`,
    // `database_type`) merely NAME a variant ("credential_chain", "duckdb") —
    // their value is never a secret. Critically, nulling such a key corrupts
    // the compiled config: e.g. `s3_secret_type` is the internal tag for the
    // ducklake `S3StorageSecret` enum, so redacting it to null makes the whole
    // untagged `DuckDBOptions` fail to deserialise at runtime → the workspace
    // 503s on the stateless fleet. Exclude `_type` keys (they'd otherwise
    // match the `secret` substring needle). See oxygen-internal#2520.
    if k.ends_with("_type") {
        return false;
    }
    const NEEDLES: &[&str] = &[
        "password",
        "passwd",
        "passphrase",
        "secret",
        "token",
        "credential",
        "private_key",
        "api_key",
        "apikey",
        "access_key",
        "secret_key",
        "account_key",
        "client_secret",
        "connection_string",
        "sas_token",
        "key_path",
        "service_account",
    ];
    NEEDLES.iter().any(|needle| k.contains(needle))
}

#[cfg(test)]
mod redact_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_inline_literals_but_keeps_var_refs() {
        let mut v = json!({
            "databases": [{
                "name": "wh",
                "password": "hunter2",
                "password_var": "WH_PASSWORD",
                "host": "db.example.com"
            }]
        });
        let n = redact_inline_secrets(&mut v);
        assert_eq!(n, 1, "only the inline password literal should be redacted");
        let db = &v["databases"][0];
        assert_eq!(db["password"], "", "inline password stripped");
        assert_eq!(db["password_var"], "WH_PASSWORD", "var reference kept");
        assert_eq!(db["host"], "db.example.com", "non-secret kept");
        assert_eq!(db["name"], "wh");
    }

    #[test]
    fn redacts_nested_and_varied_secret_keys() {
        let mut v = json!({
            "service_account_json": "{...}",
            "nested": { "client_secret": "abc", "api_key": "k", "label": "ok" },
            "empty": "",
        });
        let n = redact_inline_secrets(&mut v);
        assert_eq!(n, 3);
        assert_eq!(v["service_account_json"], "");
        assert_eq!(v["nested"]["client_secret"], "");
        assert_eq!(v["nested"]["api_key"], "");
        assert_eq!(v["nested"]["label"], "ok");
    }

    /// Regression (oxygen-internal#2520): an inline LLM `api_key` in
    /// `models` and credentials embedded in a `repositories[].git_url` URL
    /// must not survive into the compiled config.
    /// `health_check` is not a projected `CompiledConfig` column, so it must
    /// land in the `other` JSONB catch-all and survive the compile boundary
    /// without a migration. If a future change adds `health_check` to the
    /// projected keys, this test fails — keep it in `other` so the per-workspace
    /// cadence round-trips (see `internal-docs/admin-surfaces.md`,
    /// "Workspace health → who actually gets evaluated").
    #[test]
    fn health_check_lands_in_other_catch_all() {
        let yaml = json!({
            "databases": [],
            "health_check": { "interval": "30m", "enabled": true }
        });
        let cfg = build_compiled_config(yaml, None).expect("compiles");
        let other = cfg.other.expect("health_check must land in `other`");
        assert_eq!(other["health_check"]["interval"], "30m");
        assert_eq!(other["health_check"]["enabled"], true);
    }

    #[test]
    fn redacts_model_keys_and_git_url_credentials() {
        let mut models = json!([
            { "name": "local-ollama", "api_key": "sk-inline-secret" },
            { "name": "openai", "api_key_var": "OPENAI_API_KEY" },
        ]);
        let n = redact_inline_secrets(&mut models);
        assert_eq!(n, 1, "inline model api_key redacted; *_var ref kept");
        assert_eq!(models[0]["api_key"], "");
        assert_eq!(models[1]["api_key_var"], "OPENAI_API_KEY");

        let mut repos = json!([
            { "name": "dbt", "git_url": "https://x-access-token:ghp_SECRET@github.com/acme/dbt.git" },
            { "name": "clean", "git_url": "https://github.com/acme/public.git" },
        ]);
        let n = redact_inline_secrets(&mut repos);
        assert_eq!(n, 1, "only the credential-bearing git_url is rewritten");
        assert_eq!(
            repos[0]["git_url"], "https://github.com/acme/dbt.git",
            "userinfo stripped, URL still usable"
        );
        assert_eq!(
            repos[1]["git_url"], "https://github.com/acme/public.git",
            "credential-free URL untouched"
        );
    }

    /// Regression (oxygen-internal#2520): a serde internal-tag key like
    /// `s3_secret_type` (ducklake `S3StorageSecret` discriminator) must NOT be
    /// redacted just because its name contains "secret" — nulling it corrupts
    /// the compiled config so the untagged `DuckDBOptions` fails to
    /// deserialise and the workspace 503s on the fleet.
    #[test]
    fn type_discriminator_keys_are_not_redacted() {
        assert!(!is_sensitive_key("s3_secret_type"));
        assert!(!is_sensitive_key("database_type"));
        // Real secrets still redacted.
        assert!(is_sensitive_key("client_secret"));
        assert!(is_sensitive_key("secret"));
        assert!(is_sensitive_key("api_key"));

        // End-to-end: a ducklake database keeps its s3_secret_type tag.
        let mut dbs = json!([{
            "name": "ducklake",
            "type": "duckdb",
            "s3_secret_type": "credential_chain",
            "chain": "sso;config",
            "region": "us-west-2",
            "secret": "should-be-redacted"
        }]);
        redact_inline_secrets(&mut dbs);
        assert_eq!(
            dbs[0]["s3_secret_type"], "credential_chain",
            "tag preserved"
        );
        assert_eq!(dbs[0]["secret"], "", "real secret still redacted");
    }

    /// Regression (oxygen-internal#2520, follow-up): a `ManagedSecret` field
    /// holds a bare env-var reference (UPPER_SNAKE) — ducklake
    /// `secret: AWS_S3_SECRET`, `catalog_path: DUCKLAKE_CATALOG_PATH`. These
    /// NAME a secret in the encrypted store; they are structurally required and
    /// must NOT be nulled (the `s3_secret_type: config` variant won't
    /// deserialise without `secret`). Mixed-case / punctuated literals are still
    /// redacted.
    #[test]
    fn env_var_style_references_are_preserved() {
        let mut v = json!({
            "secret": "AWS_S3_SECRET",                // ManagedSecret ref → keep
            "catalog_path": "DUCKLAKE_CATALOG_PATH",  // ManagedSecret ref → keep
            "password": "hunter2",                    // literal → redact
            "api_key": "sk-abc123",                   // literal → redact
            "token": "GHP_lowerMixed",                // mixed-case literal → redact
        });
        let n = redact_inline_secrets(&mut v);
        assert_eq!(v["secret"], "AWS_S3_SECRET", "env-var ref preserved");
        assert_eq!(
            v["catalog_path"], "DUCKLAKE_CATALOG_PATH",
            "env-var ref preserved"
        );
        assert_eq!(v["password"], "", "inline literal redacted");
        assert_eq!(v["api_key"], "", "inline literal redacted");
        assert_eq!(v["token"], "", "mixed-case literal redacted");
        assert_eq!(n, 3);
    }

    /// Regression (oxygen-internal#2528): a redacted inline secret on a
    /// REQUIRED `String` field (Ollama `api_key` is `String`, not
    /// `Option<String>`) must stay a STRING — never `null`. If redaction nulls
    /// it, the compiled config fails to deserialise into the runtime `Config`,
    /// the round-trip gate rejects the revision, and the WHOLE compile fails —
    /// 0 revisions, every read 503s with `needs_recompile`. The empty string
    /// both deserialises AND is treated as absent by `resolve_config_value`, so
    /// a `*_var` fallback still resolves from the encrypted store.
    #[test]
    fn redacted_required_string_stays_a_nonempty_string() {
        let mut models = json!([{
            "name": "llama3.2",
            "vendor": "ollama",
            "api_url": "http://localhost:11434/v1",
            "api_key": "secret",
        }]);
        let n = redact_inline_secrets(&mut models);
        assert_eq!(n, 1, "the inline api_key literal is redacted");
        let redacted = &models[0]["api_key"];
        assert!(
            redacted.is_string(),
            "must stay a string so a required `String` field deserialises (was null → gate failure)"
        );
        assert_eq!(
            redacted, "",
            "empty so resolve_config_value treats it as absent and any *_var fallback still resolves"
        );
    }

    #[test]
    fn looks_like_env_var_ref_classifies() {
        assert!(looks_like_env_var_ref("AWS_S3_SECRET"));
        assert!(looks_like_env_var_ref("DUCKLAKE_CATALOG_PATH"));
        assert!(looks_like_env_var_ref("_PRIVATE"));
        assert!(looks_like_env_var_ref("X1_Y2"));
        assert!(!looks_like_env_var_ref("hunter2")); // lowercase
        assert!(!looks_like_env_var_ref("sk-abc")); // punctuation
        assert!(!looks_like_env_var_ref("Mixed_Case")); // lowercase
        assert!(!looks_like_env_var_ref("")); // empty
        assert!(!looks_like_env_var_ref("1ABC")); // leading digit
        // High-entropy uppercase blobs with NO underscore are real inline
        // secrets, not var-name references — must NOT be preserved (#2524 review).
        assert!(!looks_like_env_var_ref("JBSWY3DPEHPK3PXP")); // base32 TOTP seed
        assert!(!looks_like_env_var_ref("DEADBEEFCAFE1234")); // uppercase hex token
        assert!(!looks_like_env_var_ref("TOKEN")); // single word, no underscore
    }

    /// Golden snapshot (safety-harness item 5): pins the column split + redaction
    /// of a representative `config.yml` so any unintended change to compile
    /// OUTPUT shape shows up as a diff in review. The round-trip gate covers
    /// semantic correctness; this covers shape stability. `config.yml` is the
    /// only file kind with a non-identity transform (redaction), so it's the one
    /// that warrants a golden test.
    #[test]
    fn golden_config_compile_output() {
        let yaml = r#"
databases:
  - name: lake
    type: duckdb
    schema_name: main
    data_path: s3://bucket/lake
    s3_secret_type: config
    key_id: AKIAEXAMPLE
    secret: AWS_S3_SECRET
    region: us-west-2
  - name: pg
    type: postgres
    password: hunter2
    password_var: PG_PASSWORD
models:
  - name: openai
    api_key_var: OPENAI_API_KEY
custom_section:
  foo: bar
"#;
        let value: Value = serde_yaml::from_str(yaml).unwrap();
        let cfg = build_compiled_config(value, None).unwrap();
        let merged = entity::workspace_compiled_configs::merge_compiled_config(&cfg);
        let expected = json!({
            "databases": [
                {
                    "name": "lake",
                    "type": "duckdb",
                    "schema_name": "main",
                    "data_path": "s3://bucket/lake",
                    "s3_secret_type": "config",
                    "key_id": "AKIAEXAMPLE",
                    "secret": "AWS_S3_SECRET",
                    "region": "us-west-2"
                },
                {
                    "name": "pg",
                    "type": "postgres",
                    "password": "",
                    "password_var": "PG_PASSWORD"
                }
            ],
            "models": [
                { "name": "openai", "api_key_var": "OPENAI_API_KEY" }
            ],
            "custom_section": { "foo": "bar" }
        });
        assert_eq!(merged, expected);
    }

    #[test]
    fn strip_url_credentials_handles_edge_cases() {
        assert_eq!(strip_url_credentials("plain string"), None);
        assert_eq!(strip_url_credentials("https://host/path"), None);
        assert_eq!(
            strip_url_credentials("https://user:pass@host:5432/db").as_deref(),
            Some("https://host:5432/db")
        );
        assert_eq!(
            strip_url_credentials("postgres://u:p@h/d").as_deref(),
            Some("postgres://h/d")
        );
        // userinfo with no password (token-as-user) still stripped.
        assert_eq!(
            strip_url_credentials("https://TOKEN@github.com/x").as_deref(),
            Some("https://github.com/x")
        );
    }
}

fn compile_app(file: &DiscoveredFile, content: &str) -> Result<Vec<CompiledRow>, FileFailure> {
    let value = parse_yaml(file, content)?;
    let map = match &value {
        Value::Object(m) => m,
        _ => {
            return Err(shape_err(
                file,
                "app YAML must be a mapping at the top level",
            ));
        }
    };
    let name = map
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| derive_name_from_path(&file.rel_path));
    let published = map
        .get("published")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    Ok(vec![CompiledRow::App(CompiledApp {
        file_path: file.rel_path.clone(),
        name,
        definition: value,
        published,
    })])
}

fn compile_automation(
    file: &DiscoveredFile,
    content: &str,
    automation_kind: AutomationKind,
) -> Result<Vec<CompiledRow>, FileFailure> {
    let value = parse_yaml(file, content)?;
    let name = value
        .as_object()
        .and_then(|m| m.get("name"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| derive_name_from_path(&file.rel_path));
    Ok(vec![CompiledRow::Automation(CompiledAutomation {
        file_path: file.rel_path.clone(),
        name,
        extension: automation_kind.extension().to_string(),
        definition: value,
    })])
}

/// A `schemas/*.sql` migration. Same shape as a verified query — the
/// difference is what happens downstream: this gets applied to a tenant.
#[derive(Debug, Clone)]
pub struct CompiledSchemaMigration {
    pub file_path: String,
    pub content_sha256: String,
    pub content: String,
}

/// No parsing. The content is DDL handed to Postgres verbatim, which is the
/// point: an engineer writes what they mean and Oxy does not second-guess it.
fn compile_schema_migration(
    file: &DiscoveredFile,
    content: &str,
) -> Result<Vec<CompiledRow>, FileFailure> {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let hash: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    Ok(vec![CompiledRow::SchemaMigration(
        CompiledSchemaMigration {
            file_path: file.rel_path.clone(),
            content_sha256: hash,
            content: content.to_string(),
        },
    )])
}

/// A markdown context document, as the text an agent is given.
///
/// Never fails: there is nothing to parse, and the one thing Postgres cannot
/// store (a NUL byte) is dropped by [`crate::context_documents::document_text`]
/// rather than allowed to fail the revision. The hash is of what is stored.
fn compile_context_document(
    file: &DiscoveredFile,
    content: &str,
) -> Result<Vec<CompiledRow>, FileFailure> {
    let content = crate::context_documents::document_text(content.as_bytes());
    let hash = hex::encode(Sha256::digest(content.as_bytes()));
    Ok(vec![CompiledRow::ContextDocument(
        CompiledContextDocument {
            file_path: file.rel_path.clone(),
            content_sha256: hash,
            content,
        },
    )])
}

fn compile_verified_query(
    file: &DiscoveredFile,
    content: &str,
) -> Result<Vec<CompiledRow>, FileFailure> {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let hash: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();
    Ok(vec![CompiledRow::VerifiedQuery(CompiledVerifiedQuery {
        file_path: file.rel_path.clone(),
        content_sha256: hash,
        content: content.to_string(),
    })])
}

/// Helper for YAML kinds whose row shape is essentially
/// `(name, file_path, definition jsonb)`.
fn compile_named_yaml<F>(
    file: &DiscoveredFile,
    content: &str,
    make_row: F,
) -> Result<Vec<CompiledRow>, FileFailure>
where
    F: FnOnce(String, String, Value) -> CompiledRow,
{
    let value = parse_yaml(file, content)?;
    let map = match &value {
        Value::Object(m) => m,
        _ => return Err(shape_err(file, "YAML must be a mapping at the top level")),
    };
    let name = map
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| derive_name_from_path(&file.rel_path));
    Ok(vec![make_row(name, file.rel_path.clone(), value)])
}

fn parse_yaml(file: &DiscoveredFile, content: &str) -> Result<Value, FileFailure> {
    serde_yaml::from_str::<Value>(content).map_err(|e| FileFailure {
        path: file.rel_path.clone(),
        kind: FailureKind::Yaml,
        message: e.to_string(),
    })
}

fn shape_err(file: &DiscoveredFile, message: &str) -> FileFailure {
    FileFailure {
        path: file.rel_path.clone(),
        kind: FailureKind::Shape,
        message: message.to_string(),
    }
}

/// Strip the kind-specific suffix(es) and the trailing extension so a
/// nameless YAML row still gets a sensible identifier — e.g.
/// `agents/clarify.agentic.yml` → `agents/clarify`. Used as a fallback
/// when the `name` field is missing.
pub fn derive_name_from_path(rel_path: &str) -> String {
    const STRIPS: &[&str] = &[
        ".agentic.yml",
        ".view.yml",
        ".topic.yml",
        ".app.yml",
        ".procedure.yml",
        ".automation.yml",
        ".airway.yml",
        ".simulation.yml",
        ".yml",
    ];
    for s in STRIPS {
        if let Some(stem) = rel_path.strip_suffix(s) {
            return stem.to_string();
        }
    }
    rel_path.to_string()
}

/// Returns the identifier that would collide on the per-entity PK if
/// duplicated. Some kinds key by name; some by file_path. Config has
/// no per-row dedup (revision_id is the PK).
fn row_dedupe_key(row: &CompiledRow, _kind: &FileKind) -> Option<String> {
    match row {
        CompiledRow::Config(_) => None,
        CompiledRow::Agent(a) => Some(format!("agent:{}", a.name)),
        CompiledRow::View(v) => Some(format!("view:{}", v.name)),
        CompiledRow::Topic(t) => Some(format!("topic:{}", t.name)),
        CompiledRow::App(a) => Some(format!("app:{}", a.file_path)),
        CompiledRow::Automation(p) => Some(format!("proc:{}", p.file_path)),
        CompiledRow::VerifiedQuery(q) => Some(format!("sql:{}", q.file_path)),
        CompiledRow::SchemaMigration(m) => Some(format!("schema:{}", m.file_path)),
        CompiledRow::Pipeline(p) => Some(format!("pipe:{}", p.name)),
        CompiledRow::Reference(_) => None,
        CompiledRow::MonitorConfig(_) => None,
        CompiledRow::ReconcileConfig(_) => None,
        CompiledRow::WorldModelConfig(_) => None,
        CompiledRow::Simulation(s) => Some(format!("simulation:{}", s.name)),
        CompiledRow::ContextDocument(d) => Some(format!("doc:{}", d.file_path)),
    }
}

/// Snapshot of a successful revisions row we can reuse for an
/// idempotent compile. Carries just the fields the outcome needs;
/// the per-entity rows already exist tagged with `revision_id`.
#[derive(Debug, Clone)]
pub(crate) struct IdempotentRevision {
    pub(crate) revision_id: Uuid,
    pub(crate) started_at: chrono::DateTime<chrono::Utc>,
    pub(crate) finished_at: chrono::DateTime<chrono::Utc>,
    pub(crate) file_count_seen: u32,
    pub(crate) file_count_compiled: u32,
    pub(crate) file_count_failed: u32,
}

impl IdempotentRevision {
    pub(crate) fn into_outcome(
        self,
        git_sha: String,
        branch: Option<String>,
        promotion: Promotion,
    ) -> CompileOutcome {
        CompileOutcome {
            revision_id: self.revision_id,
            status: crate::outcome::RevisionStatus::Ready,
            git_sha,
            branch,
            started_at: self.started_at,
            finished_at: self.finished_at,
            file_count_seen: self.file_count_seen,
            file_count_compiled: self.file_count_compiled,
            file_count_failed: self.file_count_failed,
            failures: Vec::new(),
            promotion,
        }
    }
}

/// Load a revision by primary key. Window-independent — used by the
/// `SupersededBy` recovery path where the winner's `revision_id` is
/// already known from the writer's outcome. The conversion to
/// `IdempotentRevision` (the carrier type used by `into_outcome`) only
/// succeeds when the row is `ready` and has a `finished_at`; otherwise
/// returns `Ok(None)` and the caller surfaces an internal error.
async fn lookup_revision_by_id(
    db: &DatabaseConnection,
    revision_id: Uuid,
) -> Result<Option<IdempotentRevision>, CompileError> {
    use sea_orm::EntityTrait;

    let row = entity::revisions::Entity::find_by_id(revision_id)
        .one(db)
        .await?;
    Ok(row.and_then(|r| {
        if r.status != "ready" {
            return None;
        }
        let finished_at = r.finished_at.map(|f| f.with_timezone(&chrono::Utc))?;
        Some(IdempotentRevision {
            revision_id: r.revision_id,
            started_at: r.started_at.with_timezone(&chrono::Utc),
            finished_at,
            file_count_seen: r.file_count_seen.max(0) as u32,
            file_count_compiled: r.file_count_compiled.max(0) as u32,
            file_count_failed: r.file_count_failed.max(0) as u32,
        })
    }))
}

/// Look up the most recent ready `main` revision for this
/// `(workspace_id, git_sha)` within the idempotency window. Returns
/// `Ok(None)` when there's no eligible row, when the window is zero
/// (operator turned idempotency off), or when the DB hiccups (the
/// caller falls through to a full compile).
async fn lookup_idempotent_revision(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    kind: RevisionKind,
    git_sha: &str,
    compiler_version: &str,
    schema_version: i32,
) -> Result<Option<IdempotentRevision>, CompileError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};

    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(IDEMPOTENCY_WINDOW_SECS);
    let cutoff_offset = cutoff.fixed_offset();

    let row = entity::revisions::Entity::find()
        .filter(entity::revisions::Column::WorkspaceId.eq(workspace_id))
        .filter(entity::revisions::Column::GitSha.eq(git_sha))
        .filter(entity::revisions::Column::Status.eq("ready"))
        .filter(entity::revisions::Column::Kind.is_in(kind.reusable_kinds().iter().copied()))
        // Reuse is only sound when the SAME compiler produced the revision. A
        // newer binary may compile the same SHA differently — e.g. it now
        // injects the DuckDB→S3 `s3_mirror` block, or fixes a config transform.
        // Without these two filters a same-SHA re-compile within the window
        // silently reuses an OLD-binary revision, so a deploy that changes
        // compile output never takes effect until the window lapses (the
        // customer-demo "no databases configured" recurrence after the
        // s3_mirror deploy).
        .filter(entity::revisions::Column::CompilerVersion.eq(compiler_version))
        .filter(entity::revisions::Column::SchemaVersion.eq(schema_version))
        .filter(entity::revisions::Column::FinishedAt.gte(cutoff_offset))
        .order_by_desc(entity::revisions::Column::FinishedAt)
        .one(db)
        .await?;

    Ok(row.and_then(|r| {
        let finished_at = r.finished_at.map(|f| f.with_timezone(&chrono::Utc))?;
        Some(IdempotentRevision {
            revision_id: r.revision_id,
            started_at: r.started_at.with_timezone(&chrono::Utc),
            finished_at,
            file_count_seen: r.file_count_seen.max(0) as u32,
            file_count_compiled: r.file_count_compiled.max(0) as u32,
            file_count_failed: r.file_count_failed.max(0) as u32,
        })
    }))
}

/// The newest `ready` revision of `(workspace_id, git_sha)` that a compile of
/// `kind` may reuse (see [`RevisionKind::reusable_kinds`]), produced by THIS
/// compiler and schema version. Unlike the in-compile idempotency lookup there
/// is no time window: a caller asking "is this SHA already compiled?" (the
/// staging branch-compile route) wants any matching revision, however old —
/// revision ids are immutable, so an old one is exactly as correct as a new one.
pub async fn find_reusable_revision(
    db: &DatabaseConnection,
    workspace_id: Uuid,
    kind: RevisionKind,
    git_sha: &str,
) -> Result<Option<Uuid>, sea_orm::DbErr> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder};
    let kinds = kind.reusable_kinds();
    if kinds.is_empty() {
        return Ok(None);
    }
    Ok(entity::revisions::Entity::find()
        .filter(entity::revisions::Column::WorkspaceId.eq(workspace_id))
        .filter(entity::revisions::Column::GitSha.eq(git_sha))
        .filter(entity::revisions::Column::Status.eq("ready"))
        .filter(entity::revisions::Column::Kind.is_in(kinds.iter().copied()))
        .filter(entity::revisions::Column::CompilerVersion.eq(crate::compiler_version()))
        .filter(entity::revisions::Column::SchemaVersion.eq(CURRENT_SCHEMA_VERSION))
        .order_by_desc(entity::revisions::Column::FinishedAt)
        .one(db)
        .await?
        .map(|r| r.revision_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::walker::{FileKind, discover};
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    #[test]
    fn staging_kind_round_trips_and_is_never_promotable() {
        for k in [
            RevisionKind::Main,
            RevisionKind::Draft,
            RevisionKind::Staging,
        ] {
            assert_eq!(RevisionKind::parse(k.as_str()), Some(k));
        }
        assert_eq!(RevisionKind::Staging.as_str(), "staging");
        assert!(RevisionKind::Main.is_promotable());
        assert!(!RevisionKind::Draft.is_promotable());
        assert!(!RevisionKind::Staging.is_promotable());
        assert_eq!(RevisionKind::parse("bogus"), None);
    }

    #[test]
    fn a_main_compile_never_reuses_a_staging_row() {
        // The idempotent short-circuit may promote what it reuses; a main
        // compile reusing a staging row would promote a branch head.
        assert_eq!(RevisionKind::Main.reusable_kinds(), &["main"]);
        // A staging compile may reuse either: a pin reads rows by id.
        assert_eq!(RevisionKind::Staging.reusable_kinds(), &["staging", "main"]);
        assert!(RevisionKind::Draft.reusable_kinds().is_empty());
    }

    #[tokio::test]
    async fn reconcile_yml_compiles_to_reconcile_row() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(root, "reconcile.yml", "checks: []\n");
        let files = discover(root).unwrap();
        let file = files
            .iter()
            .find(|f| matches!(f.kind, FileKind::ReconcileConfig))
            .unwrap();
        let rows = compile_one(file).await.unwrap();
        assert_eq!(rows.len(), 1);
        match &rows[0] {
            CompiledRow::ReconcileConfig(c) => {
                assert!(c.definition.get("checks").unwrap().is_array());
            }
            other => panic!("expected ReconcileConfig, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn simulation_yml_compiles_to_a_named_row() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(
            root,
            "simulations/confounded.simulation.yml",
            "name: confounded_marketing\nseed: 7\nperiods: 40\n",
        );
        let files = discover(root).unwrap();
        let file = files
            .iter()
            .find(|f| matches!(f.kind, FileKind::Simulation))
            .unwrap();
        let rows = compile_one(file).await.unwrap();
        assert_eq!(rows.len(), 1);
        match &rows[0] {
            CompiledRow::Simulation(s) => {
                assert_eq!(s.name, "confounded_marketing");
                assert_eq!(s.file_path, "simulations/confounded.simulation.yml");
                assert_eq!(s.definition.get("periods").unwrap(), 40);
            }
            other => panic!("expected Simulation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_nameless_simulation_derives_its_name_from_the_path() {
        // Without the `.simulation.yml` strip, the fallback would leave the
        // literal `.simulation` on the identifier and the run's reference to
        // its world would not match what a list returned.
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(root, "worlds/flat_budget.simulation.yml", "seed: 1\n");
        let files = discover(root).unwrap();
        let file = files
            .iter()
            .find(|f| matches!(f.kind, FileKind::Simulation))
            .unwrap();
        match &compile_one(file).await.unwrap()[0] {
            CompiledRow::Simulation(s) => assert_eq!(s.name, "worlds/flat_budget"),
            other => panic!("expected Simulation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn compile_one_app_extracts_name_and_published() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(
            root,
            "apps/sales.app.yml",
            "name: Sales overview\npublished: true\ntasks: []\n",
        );
        let files = discover(root).unwrap();
        let app_file = files
            .iter()
            .find(|f| matches!(f.kind, FileKind::App))
            .unwrap();
        let rows = compile_one(app_file).await.unwrap();
        let row = &rows[0];
        match row {
            CompiledRow::App(a) => {
                assert_eq!(a.name, "Sales overview");
                assert!(a.published);
                assert_eq!(a.file_path, "apps/sales.app.yml");
            }
            other => panic!("expected App, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn compile_one_app_falls_back_to_path_name_when_unnamed() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(root, "apps/nameless.app.yml", "tasks: []\n");
        let files = discover(root).unwrap();
        let app_file = files
            .iter()
            .find(|f| matches!(f.kind, FileKind::App))
            .unwrap();
        let rows = compile_one(app_file).await.unwrap();
        let row = &rows[0];
        match row {
            CompiledRow::App(a) => {
                assert_eq!(a.name, "apps/nameless");
                assert!(!a.published);
            }
            other => panic!("expected App, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn compile_one_yaml_failure_returns_structured_error() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(
            root,
            "agents/broken.agentic.yml",
            "this: is: invalid: yaml:\n",
        );
        let files = discover(root).unwrap();
        let file = files
            .iter()
            .find(|f| matches!(f.kind, FileKind::AgenticAgent))
            .unwrap();
        let err = compile_one(file).await.unwrap_err();
        assert!(matches!(err.kind, FailureKind::Yaml));
        assert_eq!(err.path, "agents/broken.agentic.yml");
    }

    #[tokio::test]
    async fn compile_one_verified_query_records_sha256() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(root, "queries/top.sql", "SELECT 1\n");
        let files = discover(root).unwrap();
        let file = files
            .iter()
            .find(|f| matches!(f.kind, FileKind::VerifiedQuery))
            .unwrap();
        let rows = compile_one(file).await.unwrap();
        match &rows[0] {
            CompiledRow::VerifiedQuery(q) => {
                assert_eq!(q.file_path, "queries/top.sql");
                assert_eq!(q.content, "SELECT 1\n");
                // sha256("SELECT 1\n") — sanity check, not pinned.
                assert_eq!(q.content_sha256.len(), 64);
                assert!(q.content_sha256.chars().all(|c| c.is_ascii_hexdigit()));
            }
            other => panic!("expected VerifiedQuery, got {:?}", other),
        }
    }

    /// The body is carried verbatim with its hash, and a document nobody
    /// references is not a row at all.
    #[tokio::test]
    async fn compile_one_context_document_carries_the_body_verbatim() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        let body = "# Glossary\n\n**GMV** is gross merchandise value.\n";
        write(root, "docs/glossary.md", body);
        write(root, "README.md", "# nobody's context reaches this\n");
        write(
            root,
            "analyst.agentic.yml",
            "name: analyst\ncontext:\n  - ./docs/*.md\n",
        );

        let documents: Vec<_> = discover(root)
            .unwrap()
            .into_iter()
            .filter(|f| matches!(f.kind, FileKind::ContextDocument))
            .collect();
        assert_eq!(documents.len(), 1, "the README is not a context document");

        let rows = compile_one(&documents[0]).await.unwrap();
        match &rows[0] {
            CompiledRow::ContextDocument(d) => {
                assert_eq!(d.file_path, "docs/glossary.md");
                assert_eq!(d.content, body);
                assert_eq!(
                    d.content_sha256,
                    hex::encode(Sha256::digest(body.as_bytes()))
                );
                assert_eq!(
                    row_dedupe_key(&rows[0], &documents[0].kind).as_deref(),
                    Some("doc:docs/glossary.md")
                );
            }
            other => panic!("expected ContextDocument, got {:?}", other),
        }
    }

    /// Postgres `TEXT` cannot hold a NUL, and any file failure fails the whole
    /// revision. So a NUL is dropped rather than refused: one odd `.md` under
    /// a broad glob must not stop every promotion of the workspace.
    #[tokio::test]
    async fn a_nul_byte_in_a_context_document_is_dropped_not_a_failed_revision() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(root, "docs/odd.md", "looks like text\0and mostly is");
        write(
            root,
            "analyst.agentic.yml",
            "name: analyst\ncontext:\n  - ./docs/*.md\n",
        );

        let file = discover(root)
            .unwrap()
            .into_iter()
            .find(|f| matches!(f.kind, FileKind::ContextDocument))
            .expect("the document is discovered");
        let rows = compile_one(&file)
            .await
            .expect("a NUL does not fail the file");
        match &rows[0] {
            CompiledRow::ContextDocument(d) => {
                assert_eq!(d.content, "looks like textand mostly is");
                assert_eq!(
                    d.content_sha256,
                    hex::encode(Sha256::digest(d.content.as_bytes())),
                    "the hash is of what is stored"
                );
            }
            other => panic!("expected ContextDocument, got {:?}", other),
        }
    }

    #[test]
    fn derive_name_from_path_strips_known_suffixes() {
        assert_eq!(
            derive_name_from_path("agents/foo.agentic.yml"),
            "agents/foo"
        );
        assert_eq!(derive_name_from_path("views/v.view.yml"), "views/v");
        assert_eq!(derive_name_from_path("topics/t.topic.yml"), "topics/t");
        assert_eq!(derive_name_from_path("apps/a.app.yml"), "apps/a");
        assert_eq!(derive_name_from_path("p/x.procedure.yml"), "p/x");
        assert_eq!(derive_name_from_path("p/z.automation.yml"), "p/z");
        assert_eq!(derive_name_from_path("pipe/a.airway.yml"), "pipe/a");
        assert_eq!(derive_name_from_path("noext"), "noext");
    }
}
