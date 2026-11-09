//! `oxy compile` — Phase 1.6a entry point for the compile boundary.
//!
//! Walks the local workspace, parses every recognised YAML/SQL file,
//! and writes the result into the compile-boundary Postgres schema
//! (revisions + per-entity tables) as a single revision_id.
//! Phase 1.6a does NOT update `workspaces.current_revision_id`;
//! observation only.
//!
//! Requires `OXY_DATABASE_URL` set (same as `oxy worker` and the
//! cloud-mode `oxy serve`). Local-mode users running `oxy start`
//! already have an embedded Postgres available.

use std::path::PathBuf;

use clap::Parser;
use oxy::config::resolve_local_workspace_path;
use oxy::database::client::establish_connection;
use oxy::theme::StyledText;
use oxy_compile::{
    CompileError, CompileOutcome, CompileRequest, FailureKind, RevisionKind, RevisionStatus,
    compile_workspace, compiler_version,
};
use oxy_shared::errors::OxyError;
use uuid::Uuid;

use super::serve;

#[derive(Parser, Debug, Clone)]
pub struct CompileArgs {
    /// Path to the workspace directory. Defaults to walking up from
    /// the current directory until a `config.yml` is found (same
    /// resolution as `oxy build`).
    #[clap(long)]
    pub workspace_path: Option<PathBuf>,

    /// Workspace UUID to associate the compile with. Defaults to the
    /// nil UUID, which is the local-mode workspace ID (matches the
    /// row seeded by `oxy start` / `oxy serve --local`). In cloud
    /// mode this must be supplied.
    #[clap(long)]
    pub workspace_id: Option<Uuid>,

    /// Optional git SHA recorded on the revision. When omitted, the
    /// literal "local" is used — useful for working-copy compiles
    /// with uncommitted edits.
    #[clap(long)]
    pub git_sha: Option<String>,

    /// Optional branch name recorded on the revision.
    #[clap(long)]
    pub branch: Option<String>,

    /// Skip running database migrations. Use when another process
    /// (`oxy serve`, CI step) has already migrated the database.
    #[clap(long)]
    pub skip_migrations: bool,

    /// Print the structured outcome as JSON on stdout instead of the
    /// human summary. Useful for CI scripts.
    #[clap(long)]
    pub json: bool,

    /// Atomically promote the new revision into
    /// `workspaces.current_revision_id` when the compile succeeds.
    /// Off by default so a manual `oxy compile` is a dry run — it
    /// proves the workspace compiles without moving what the runtime
    /// serves. The worker and the CLI's production paths pass it.
    #[clap(long)]
    pub promote: bool,

    /// Run the underlying enterprise migrations alongside the OSS set.
    /// Mirrors `oxy serve --enterprise`; defaults to false.
    #[clap(long)]
    pub enterprise: bool,
}

/// `oxy compile` handler.
#[tracing::instrument(skip_all)]
pub async fn run_compile(args: CompileArgs) -> Result<(), OxyError> {
    let workspace_path = match args.workspace_path.clone() {
        Some(p) => p,
        None => resolve_local_workspace_path()?,
    };

    if !workspace_path.exists() {
        return Err(OxyError::RuntimeError(format!(
            "workspace path not found: {}",
            workspace_path.display()
        )));
    }

    if args.skip_migrations || serve::skip_migrations_requested() {
        tracing::info!("compile: skipping migrations (--skip-migrations or OXY_SKIP_MIGRATIONS)");
    } else {
        tracing::info!("compile: running database migrations");
        serve::run_database_migrations(args.enterprise).await?;
    }

    let db = establish_connection()
        .await
        .map_err(|e| OxyError::RuntimeError(format!("database connection failed: {e}")))?;

    // The config gate below reads the rollout flags (`compile_refuse_*`), and
    // an unloaded cache reads them at their registry default — off — whatever
    // the admin page says. One load, no refresh: this process compiles once
    // and exits. Fail-fast, as `oxy serve` and `oxy worker` do.
    crate::server::feature_flags::cache::load_once().await?;

    let workspace_id = args.workspace_id.unwrap_or(Uuid::nil());

    println!(
        "{} {}",
        "Compiling workspace".text(),
        format!("id={} path={}", workspace_id, workspace_path.display()).secondary()
    );

    let outcome = compile_workspace(CompileRequest {
        db: &db,
        workspace_id,
        workspace_path: &workspace_path,
        git_sha: args.git_sha.clone(),
        branch: args.branch.clone(),
        compiler_version: compiler_version(),
        promote: args.promote,
        kind: RevisionKind::Main,
        owner_user_id: None,
        // Same read-time deserialisation gate the worker installs, so `oxy
        // compile` on a working copy catches an unreadable config locally too.
        config_gate: Some(crate::server::compile_config_gate::runtime_config_gate()),
    })
    .await
    .map_err(compile_error_to_oxy)?;

    // Apply-then-promote. A revision carrying `schemas/*.sql` comes back
    // unpromoted; this applies the DDL to the org's database and then moves
    // `current_revision_id`. A no-op for a workspace with no `schemas/`
    // directory, which is every workspace that has not opted into OLTP.
    let settled =
        crate::server::compile_oltp::settle_deferred_promotion(&db, workspace_id, &outcome).await;
    // Fold the settle result back in before anything reports the outcome.
    // `outcome.promotion` is what the COMPILER decided, so for a deferred
    // revision it still says `Deferred` — and `--json` would report that for a
    // revision whose DDL has since landed and which is already live.
    let outcome = CompileOutcome {
        promotion: settled.effective_promotion(&outcome.promotion),
        ..outcome
    };

    if args.json {
        let body = serde_json::to_string_pretty(&outcome)
            .map_err(|e| OxyError::RuntimeError(format!("serialize outcome failed: {e}")))?;
        println!("{}", body);
    } else {
        print_human_summary(&outcome);
        let note = settled.summary();
        if !note.is_empty() {
            println!("{}", note.secondary());
        }
    }

    // Non-zero exit when the compile recorded any failures.
    if matches!(outcome.status, RevisionStatus::Failed) {
        return Err(OxyError::RuntimeError(format!(
            "compile recorded {} failed file(s); revision_id={}",
            outcome.file_count_failed, outcome.revision_id
        )));
    }

    // …or when every file compiled but its DDL would not apply. Exiting 0
    // here would tell CI the deploy is live when the workspace is still
    // serving the previous revision.
    if !settled.compile_succeeded() {
        return Err(OxyError::RuntimeError(format!(
            "{}; revision_id={}",
            settled.summary(),
            outcome.revision_id
        )));
    }

    Ok(())
}

fn print_human_summary(outcome: &CompileOutcome) {
    let duration = outcome
        .finished_at
        .signed_duration_since(outcome.started_at);
    let duration_ms = duration.num_milliseconds().max(0);

    let status_line = match outcome.status {
        RevisionStatus::Ready => format!(
            "{} {} files compiled in {}ms",
            "✓ Compile succeeded".success(),
            outcome.file_count_compiled,
            duration_ms
        ),
        RevisionStatus::Failed => format!(
            "{} {} of {} files failed in {}ms",
            "✗ Compile recorded failures".error(),
            outcome.file_count_failed,
            outcome.file_count_seen,
            duration_ms
        ),
    };
    println!("{}", status_line);
    println!(
        "  revision_id  = {}",
        outcome.revision_id.to_string().tertiary()
    );
    println!("  git_sha      = {}", outcome.git_sha.tertiary());
    if let Some(branch) = &outcome.branch {
        println!("  branch       = {}", branch.tertiary());
    }
    println!("  files_seen   = {}", outcome.file_count_seen);
    println!("  files_done   = {}", outcome.file_count_compiled);
    println!("  files_failed = {}", outcome.file_count_failed);

    if !outcome.failures.is_empty() {
        println!("\n{}", "Failures:".error());
        for failure in &outcome.failures {
            println!(
                "  {} {} — {}",
                kind_label(&failure.kind).tertiary(),
                failure.path,
                failure.message
            );
        }
    }
}

fn kind_label(kind: &FailureKind) -> &'static str {
    match kind {
        FailureKind::Yaml => "[yaml]",
        FailureKind::Io => "[io]",
        FailureKind::Shape => "[shape]",
        FailureKind::Duplicate => "[dup]",
        FailureKind::Validation => "[invalid]",
    }
}

fn compile_error_to_oxy(e: CompileError) -> OxyError {
    OxyError::RuntimeError(format!("compile failed: {e}"))
}
