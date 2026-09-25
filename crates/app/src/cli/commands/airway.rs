//! `oxy airway` CLI — run airway ELT pipelines.
//!
//! Shares the same domain path as the HTTP `/agentic-airway/runs`
//! endpoint via `agentic_pipeline::airway_run`: seed the queue, drive
//! the coordinator + worker, stream events to the terminal.

use std::sync::Arc;
use std::time::Duration;

use agentic_pipeline::airway_run::{
    AirwayRunError, StartAirwayRequest, spawn_airway_run_drive, start_airway_run,
};
use agentic_pipeline::backfill::{
    ChunkDisposition, ChunkGranularity, ChunkProgress, drive_backfill_range,
    find_or_create_backfill_range, load_coverage,
};
use agentic_pipeline::{
    AIRWAY_SOURCE_TYPE, AirwayMigrator, AnalyticsMigrator, AutomationMigrator, airway_event_handler,
};
use agentic_runtime::crud;
use agentic_runtime::event_registry::EventRegistry;
use agentic_runtime::migration::RuntimeMigrator;
use agentic_runtime::state::RuntimeState;
use chrono::{DateTime, Utc};
use clap::Parser;
use migration::MigratorTrait;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::config::resolve_local_workspace_path;
use oxy::database::client::establish_connection;
use oxy::theme::StyledText;
use oxy_shared::errors::OxyError;
use sea_orm::DatabaseConnection;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};
use uuid::Uuid;

#[derive(Parser, Debug)]
pub struct AirwayArgs {
    #[clap(subcommand)]
    pub command: AirwayCommand,
}

#[derive(Parser, Debug)]
pub enum AirwayCommand {
    /// Run an airway pipeline defined by a `.airway.yml` file.
    Run(AirwayRunArgs),
    /// Chunked, resumable backfill of a pipeline over a date range. Splits
    /// `[from, to)` into period chunks, runs each as a bounded backfill, and
    /// records every chunk in `backfill_checkpoints` — so a crash/cancel resumes
    /// by skipping `done` chunks and re-running failed ones.
    Backfill(AirwayBackfillArgs),
    /// Show backfill coverage for a pipeline (done / failed / pending chunks)
    /// from `backfill_checkpoints` — i.e. what period is loaded vs missing.
    Coverage(AirwayCoverageArgs),
    /// List the single-flight leases held in a workspace — the answer to
    /// "why won't this pipeline start?".
    Leases(AirwayLeasesArgs),
    /// Force-release a pipeline's single-flight lease.
    ///
    /// The recovery path for a holder that will never release itself: a
    /// dead-lettered run (the reaper cannot free it), or a `Ctrl-C`'d
    /// `oxy airway run`, which otherwise leaves the pipeline unrunnable until
    /// the 6h TTL lapses.
    ReleaseLease(AirwayReleaseLeaseArgs),
}

#[derive(Parser, Debug)]
pub struct AirwayLeasesArgs {
    /// Workspace to inspect. Defaults to the local single-tenant workspace
    /// (`Uuid::nil()`), the legacy-local default; pass `--workspace-id` for a cloud lease.
    ///
    /// Required for cloud: a lease stranded by a dead-lettered run lives under
    /// a real workspace id, and without this flag the recovery command cannot
    /// name the case it exists for — it would list the local workspace and
    /// report "no leases held" while prod stays blocked for six hours.
    #[clap(long)]
    pub workspace_id: Option<Uuid>,
}

#[derive(Parser, Debug)]
pub struct AirwayReleaseLeaseArgs {
    /// Pipeline NAME (the `name:` in the `.airway.yml`), not the file path —
    /// leases are keyed by name, which is what `oxy airway leases` prints.
    pub pipeline_name: String,
    /// Skip the confirmation prompt, and release whatever holds the lease
    /// rather than only the run that was listed.
    #[clap(long)]
    pub force: bool,
    /// Workspace holding the lease. Defaults to the local single-tenant
    /// workspace; required to clear a cloud lease. See `AirwayLeasesArgs`.
    #[clap(long)]
    pub workspace_id: Option<Uuid>,
}

#[derive(Parser, Debug)]
pub struct AirwayRunArgs {
    /// Path to the `.airway.yml`, relative to the workspace root.
    pub pipeline_ref: String,
    /// Emit one JSON object per event instead of pretty output.
    #[clap(long)]
    pub json: bool,
    /// Workspace to run as. Defaults to the local workspace (`Uuid::nil()`).
    ///
    /// Destinations that resolve per tenant need a real one: a
    /// `postgres_managed` database maps workspace → org → `oltp_tenants`, and
    /// the nil workspace belongs to no org, so the run fails with "not a known
    /// config.yml database with an airway-writable type" — which reads like a
    /// config error rather than a missing workspace. Matches `--workspace-id`
    /// on `leases` / `release-lease`.
    #[clap(long)]
    pub workspace_id: Option<Uuid>,
}

#[derive(Parser, Debug)]
pub struct AirwayBackfillArgs {
    /// Path to the `.airway.yml`, relative to the workspace root.
    pub pipeline_ref: String,
    /// Inclusive window start (RFC3339, e.g. `2020-01-01T00:00:00Z`).
    #[clap(long)]
    pub from: String,
    /// Exclusive window end (RFC3339).
    #[clap(long)]
    pub to: String,
    /// Chunk size: `month` (default), `week`, or `day`.
    #[clap(long, default_value = "month")]
    pub granularity: String,
    /// Resources to replay (repeatable). Omitted means every resource.
    ///
    /// Worth setting on any source with SNAPSHOT resources. A backfill run is
    /// run-scoped, so a snapshot reads empty state, concludes it has never run,
    /// and pulls its ordinary daily snapshot on EVERY chunk — for a period the
    /// upstream serves no historical form of, against a request quota the
    /// windowed resources need. The scope is stored on the range, so a later
    /// resume keeps it, and changing it starts a new range rather than
    /// resuming the old one under a different scope.
    #[clap(long = "resources", value_delimiter = ',')]
    pub resources: Vec<String>,
    /// Max chunks to run concurrently.
    ///
    /// FORCED TO 1. Kept as a flag so existing invocations don't break, but a
    /// higher value is ignored — concurrent chunks of one pipeline are unsafe
    /// for three reasons, only the first of which was previously understood:
    ///
    ///  1. All chunks append to the SAME `<schema>_raw.<table>`, and the fold's
    ///     watermark is `max(_aw_ingested_at)` over that whole buffer — so one
    ///     chunk's fold folds and DRAINS another chunk's partially-loaded rows
    ///     mid-flight. Not data loss (rows are valid, deduped by guid), but the
    ///     bounded-window abstraction leaks entirely.
    ///  2. Concurrent folds of one table is the exact shape of the duplicate
    ///     rows measured on the pokehouse tenant (152 excess rows, every pair
    ///     spanning two `_aw_load_id`s). Why the cross-pod advisory lock did
    ///     not prevent those is still unexplained, so running 4 folds in
    ///     parallel is doing the suspected-harmful thing on purpose.
    ///  3. Backfills pull OLD windows, and the fold's version guard is merged
    ///     but not in the pinned airway release — so today an older re-pulled
    ///     row overwrites a newer one. Concurrency multiplies that exposure.
    ///
    /// Cursor state IS isolated per chunk (run-scoped store), which is what
    /// made this look safe; that isolation covers the cursor and nothing else.
    #[clap(long, default_value = "1")]
    pub concurrency: usize,
    /// Emit one JSON object per event instead of pretty output.
    #[clap(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct AirwayCoverageArgs {
    /// Path to the `.airway.yml`, relative to the workspace root.
    pub pipeline_ref: String,
    /// Emit JSON instead of a table.
    #[clap(long)]
    pub json: bool,
}

pub async fn handle_airway_command(args: AirwayArgs) -> Result<(), OxyError> {
    match args.command {
        AirwayCommand::Run(a) => cmd_run(a).await,
        AirwayCommand::Backfill(a) => cmd_backfill(a).await,
        AirwayCommand::Coverage(a) => cmd_coverage(a).await,
        AirwayCommand::Leases(a) => cmd_leases(a).await,
        AirwayCommand::ReleaseLease(a) => cmd_release_lease(a).await,
    }
}

async fn connect_db() -> Result<sea_orm::DatabaseConnection, OxyError> {
    let db = establish_connection().await?;
    migration::Migrator::up(&db, None)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("migrations: {e}")))?;
    RuntimeMigrator::up(&db, None)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("runtime migrations: {e}")))?;
    AnalyticsMigrator::up(&db, None)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("analytics migrations: {e}")))?;
    AutomationMigrator::up(&db, None)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("workflow migrations: {e}")))?;
    AirwayMigrator::up(&db, None)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("airway migrations: {e}")))?;

    // The one connect path `run`, `backfill` and `coverage` all share, so it
    // is the seam that covers the whole `oxy airway` surface. Installs
    // airway's process-wide `GlobalConfig` from `airway_deployment_config`
    // before any connector exists — `run_pipeline` would install it too, but
    // only for the runs, and only after this command has already had a chance
    // to build one. Never fails the command; see `crate::airway_boot`.
    crate::airway_boot::install_deployment_tier(Some(&db)).await;

    Ok(db)
}

fn build_event_registry() -> EventRegistry {
    let mut registry = EventRegistry::new();
    registry.register(AIRWAY_SOURCE_TYPE, airway_event_handler());
    registry
}

/// Statuses that mean the run is over and the CLI should stop polling.
///
/// This mirrors the runtime's own definition — the set `transition_run` clears
/// the driver lease on (`agentic-runtime`'s `lifecycle::crud`). Note what is
/// NOT here: `completed_with_errors` is a *backfill-range* status derived by
/// `classify_run_outcome`, never a value of `agentic_runs.task_status`, so a
/// partial failure reaches this loop as plain `done`.
const TERMINAL_STATUSES: &[&str] = &["done", "failed", "cancelled", "timed_out"];

/// The subset of [`TERMINAL_STATUSES`] that must exit non-zero.
const FAILED_STATUSES: &[&str] = &["failed", "cancelled", "timed_out"];

fn is_terminal(status: Option<&str>) -> bool {
    status.is_some_and(|s| TERMINAL_STATUSES.contains(&s))
}

/// Build the airway `variables` map from the process environment,
/// filtered to only the variables the rendered YAML actually references
/// (via a minijinja undeclared-vars scan).
///
/// Why filter: the variables we return get persisted in
/// `agentic_task_queue` AND the run-metadata JSONB. Passing the whole
/// `std::env::vars()` would bleed unrelated secrets into both stores
/// indefinitely (AWS_SECRET_ACCESS_KEY, OPENAI_API_KEY, OXY_DATABASE_URL,
/// the entire *_var surface), bypassing the *_var → secret-manager →
/// strip flow the rest of agentic-pipeline maintains. Scanning the YAML
/// for the variables it actually uses keeps the env-pass-through useful
/// (`{{ YELP_API_KEY }}` still works) without bleeding everything else
/// into storage.
///
/// On any failure (file not readable, template not compilable), we
/// return an empty map. That surfaces as a clear "airway variable
/// substitution failed: undefined variable" at render time if the YAML
/// references something, instead of silently regressing to the old
/// "pass everything" behaviour.
fn build_env_vars_for_yaml(
    project_path: &std::path::Path,
    pipeline_ref: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let yaml_path = project_path.join(pipeline_ref);
    let yaml = match std::fs::read_to_string(&yaml_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %yaml_path.display(),
                "env-var scan: could not read pipeline YAML; passing no env vars. \
                 If the pipeline references `{{ FOO }}` you'll see a substitution \
                 error at render time."
            );
            return serde_json::Map::new();
        }
    };

    // `track_nested = true` returns root variable names for nested accesses
    // (`{{ config.api.key }}` → returns `config`). For env vars that's
    // exactly what we want — process env is flat-keyed.
    let env = minijinja::Environment::new();
    let referenced: std::collections::HashSet<String> =
        match env.template_from_named_str("pipeline", &yaml) {
            Ok(tpl) => tpl.undeclared_variables(true).into_iter().collect(),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %yaml_path.display(),
                    "env-var scan: minijinja could not parse pipeline YAML as a \
                     template; passing no env vars."
                );
                return serde_json::Map::new();
            }
        };

    std::env::vars()
        .filter(|(k, _)| referenced.contains(k))
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect()
}

async fn cmd_run(args: AirwayRunArgs) -> Result<(), OxyError> {
    let db = connect_db().await?;
    let project_path = resolve_local_workspace_path()?;
    let workspace_id = args.workspace_id.unwrap_or_else(Uuid::nil);
    let workspace_manager = WorkspaceBuilder::new(workspace_id)
        .with_working_copy(&project_path, None, oxy::config::OnMissing::Fail)
        .await?
        .build()
        .await?;

    let project_ctx = Arc::new(crate::agentic_wiring::OxyProjectContext::new(
        workspace_manager,
    ));
    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = project_ctx.clone();
    let workspace: Arc<dyn agentic_pipeline::WorkflowWorkspaceContext> = project_ctx;

    // Surface process env vars into the airway minijinja context so
    // pipeline YAMLs can reference secrets with `{{ MY_API_KEY }}`. The
    // CLI already loaded `.env` via `dotenv::from_path` higher up the
    // binary, so this picks up project-local secrets too without
    // re-implementing dotenv parsing. The HTTP path receives variables
    // from the request body and is unaffected.
    //
    // ⚠ Filter to only the variables the YAML actually references via
    // minijinja's undeclared-vars scan. Dumping the whole process env
    // unfiltered would persist unrelated secrets (AWS_SECRET_ACCESS_KEY,
    // OPENAI_API_KEY, OXY_DATABASE_URL, …) in `agentic_task_queue` and
    // run-metadata JSONB — both indefinitely retained at rest, both
    // bypassing the `*_var` → secret-manager → strip flow the rest of
    // the pipeline maintains. Scanning the rendered YAML keeps the
    // env-pass-through useful (`{{ YELP_API_KEY }}` still works) without
    // bleeding everything else into storage.
    let env_vars: serde_json::Map<String, serde_json::Value> =
        build_env_vars_for_yaml(&project_path, &args.pipeline_ref);
    let request = StartAirwayRequest {
        pipeline_ref: args.pipeline_ref.clone(),
        variables: Some(serde_json::Value::Object(env_vars)),
        thread_id: None,
        resources: Vec::new(),
        schedule_id: None,
        trigger: None,
        logical_date: None,
        retry_of: None,
        backfill_from: None,
        backfill_to: None,
    };

    // Single-process CLI: a co-located scoped coordinator drives this run.
    //
    // The SAME `workspace_id` the WorkspaceBuilder above got. This passed
    // `Uuid::nil()` unconditionally, so `--workspace-id X` built the workspace
    // as X and then started the run as nil — and that argument is not
    // cosmetic. It picks the admission policy (`resolve_admission`), it is the
    // key `find_coalescible_run` and the pipeline lease match on, and it is
    // what the run row is attributed to. A run under nil therefore could not
    // see a concurrent run on X, bypassing the "one run per pipeline per
    // workspace" guarantee, and landed in history and Workspace Health under
    // the wrong tenant.
    let run_id = match start_airway_run(
        &db,
        workspace.as_ref(),
        request,
        agentic_pipeline::TaskScope::Scoped,
        workspace_id,
    )
    .await
    {
        Ok(id) => id,
        Err(AirwayRunError::InvalidInput(msg)) | Err(AirwayRunError::Io(msg)) => {
            return Err(OxyError::ConfigurationError(msg));
        }
        Err(AirwayRunError::Airway(e)) => {
            return Err(OxyError::ConfigurationError(format!("airway spec: {e}")));
        }
        Err(e) => return Err(OxyError::RuntimeError(format!("start: {e}"))),
    };

    if !args.json {
        println!("{}", format!("Airway run started: {run_id}").info());
    }

    let state = Arc::new(RuntimeState::new());
    let (_answer_tx, _answer_rx) = mpsc::channel::<String>(1);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    // Airway has no HITL; only the cancel side of the pair is wired.
    state.register(&run_id, mpsc::channel::<String>(1).0, cancel_tx);

    spawn_airway_run_drive(
        db.clone(),
        Arc::clone(&state),
        run_id.clone(),
        platform,
        cancel_rx,
        // One-shot CLI: no long-lived LISTEN/NOTIFY router needed; the
        // worker's backstop poll claims the single queued task.
        Arc::new(agentic_runtime::router::NoopTaskRouter),
    );

    let registry = build_event_registry();
    let mut processor = registry.stream_processor(AIRWAY_SOURCE_TYPE);
    let mut last_seq: i64 = -1;

    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;

        let rows = crud::get_events_after(&db, &run_id, last_seq)
            .await
            .unwrap_or_default();
        for row in rows {
            last_seq = row.seq;
            for (event_type, payload) in processor.process(&row.event_type, &row.payload) {
                emit(args.json, row.seq, &event_type, &payload);
            }
        }

        // Termination is keyed off the run row's task_status rather than
        // a join handle — `spawn_airway_run_drive` owns its tasks
        // internally and doesn't hand one back.
        let final_status = match crud::get_run(&db, &run_id).await {
            Ok(Some(run)) if is_terminal(run.task_status.as_deref()) => {
                Some(run.task_status.unwrap_or_default())
            }
            // The run row is gone. Nothing left to wait for, and nothing to
            // report a status from.
            Ok(None) => Some(String::new()),
            _ => None,
        };
        if let Some(status) = final_status {
            // Final sweep so the last batch of events isn't dropped.
            let rows = crud::get_events_after(&db, &run_id, last_seq)
                .await
                .unwrap_or_default();
            for row in rows {
                for (event_type, payload) in processor.process(&row.event_type, &row.payload) {
                    emit(args.json, row.seq, &event_type, &payload);
                }
            }
            // Exit non-zero on a failed run.
            //
            // This printed `✗ pipeline error: …` and then returned `Ok(())`, so
            // `oxy airway run` exited 0 on a pipeline that landed nothing. Every
            // caller that checks an exit code — CI, a cron wrapper, `set -e`,
            // the OLTP demo script — read a failure as a success, and the only
            // way to tell was to grep stdout for a mark. A CLI whose failures
            // are invisible to `$?` is one nobody notices breaking.
            //
            // Which statuses are failures lives in `FAILED_STATUSES`, a
            // declared subset of the terminal set, so a status can never end
            // the poll loop without also being classified here.
            return if FAILED_STATUSES.contains(&status.as_str()) {
                Err(OxyError::RuntimeError(format!(
                    "airway run {run_id} {status} — see the events above"
                )))
            } else {
                Ok(())
            };
        }
    }
}

// ─── chunked backfill (resumable) ───────────────────────────────────────────
//
// The orchestration (chunk enumeration, per-chunk windowed runs, checkpointing,
// coverage rollup) lives in `agentic_pipeline::backfill` so the HTTP
// `/agentic-airway/chunked-backfill` + `/coverage` handlers share it. The CLI
// here is a thin wrapper: build the local context, drive, print.

fn parse_rfc3339(s: &str, field: &str) -> Result<DateTime<Utc>, OxyError> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|e| OxyError::ConfigurationError(format!("invalid --{field} `{s}`: {e}")))
}

/// Set up the same db + platform/workspace context `cmd_run` uses, plus the
/// pipeline's env-var variables, so a chunk run is identical to a normal run.
async fn airway_context(
    pipeline_ref: &str,
) -> Result<
    (
        DatabaseConnection,
        Arc<dyn agentic_pipeline::platform::PlatformContext>,
        Arc<dyn agentic_pipeline::WorkflowWorkspaceContext>,
        Value,
    ),
    OxyError,
> {
    let db = connect_db().await?;
    let project_path = resolve_local_workspace_path()?;
    let workspace_manager = WorkspaceBuilder::new(Uuid::nil())
        .with_working_copy(&project_path, None, oxy::config::OnMissing::Fail)
        .await?
        .build()
        .await?;
    let project_ctx = Arc::new(crate::agentic_wiring::OxyProjectContext::new(
        workspace_manager,
    ));
    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = project_ctx.clone();
    let workspace: Arc<dyn agentic_pipeline::WorkflowWorkspaceContext> = project_ctx;
    let variables = Value::Object(build_env_vars_for_yaml(&project_path, pipeline_ref));
    Ok((db, platform, workspace, variables))
}

async fn cmd_backfill(args: AirwayBackfillArgs) -> Result<(), OxyError> {
    let granularity = ChunkGranularity::parse(&args.granularity).ok_or_else(|| {
        OxyError::ConfigurationError(format!(
            "invalid --granularity `{}` (expected month|week|day)",
            args.granularity
        ))
    })?;
    let from = parse_rfc3339(&args.from, "from")?;
    let to = parse_rfc3339(&args.to, "to")?;
    if from >= to {
        return Err(OxyError::ConfigurationError(
            "empty window — --from must be before --to".to_string(),
        ));
    }

    let (db, platform, _workspace, variables) = airway_context(&args.pipeline_ref).await?;
    if !args.json {
        println!(
            "{}",
            format!(
                "Backfill {} ({} chunks)",
                args.pipeline_ref, args.granularity
            )
            .info()
        );
    }

    // Single-tenant local: LOCAL_WORKSPACE_ID (Uuid::nil()), no initiating user.
    // find-or-create so re-running the same window resumes that range (drives
    // only its not-`done` chunks) instead of spawning a duplicate.
    let range_id = find_or_create_backfill_range(
        &db,
        Uuid::nil(),
        &args.pipeline_ref,
        from,
        to,
        granularity,
        args.concurrency as i32,
        None,
        &args.resources,
    )
    .await
    .map_err(|e| OxyError::RuntimeError(format!("resolve backfill range: {e}")))?;

    let json = args.json;
    let summary = drive_backfill_range(
        &db,
        platform,
        range_id,
        Some(variables),
        |p: ChunkProgress| {
            if json {
                return;
            }
            match p.disposition {
                ChunkDisposition::Resumed => {
                    println!("  {}", format!("↪ {} (already done)", p.label).secondary())
                }
                ChunkDisposition::Done => println!("  {}", format!("✓ {}", p.label).success()),
                ChunkDisposition::Degraded | ChunkDisposition::Failed => {
                    let note = p.note.unwrap_or_default();
                    println!("  {}", format!("✗ {} ({note})", p.label).error());
                }
                // Not a failure: the chunk was never attempted because another
                // run held the lease, and it stays pending for the next pass.
                // Printed distinctly so an operator scanning output doesn't
                // read a deferral as an error.
                ChunkDisposition::Deferred => {
                    let note = p.note.unwrap_or_default();
                    println!("  {}", format!("⏸ {} ({note})", p.label).warning());
                }
            }
        },
    )
    .await
    .map_err(|e| OxyError::RuntimeError(format!("chunked backfill: {e}")))?;

    println!(
        "{}",
        format!(
            "done={} resumed={} degraded={} failed={} deferred={}",
            summary.done, summary.resumed, summary.degraded, summary.failed, summary.deferred
        )
        .info()
    );
    // `deferred` counts toward the re-run hint. Without it an all-deferred pass
    // printed every counter as zero, gave no hint and exited 0 — a backfill that
    // did nothing, reported as a backfill that had nothing to do.
    if summary.degraded + summary.failed > 0 {
        println!(
            "{}",
            "re-run the same command to retry failed / partial chunks.".secondary()
        );
    }
    if summary.deferred > 0 {
        println!(
            "{}",
            format!(
                "{} chunk(s) deferred — another run held the pipeline's lease. Re-run once it finishes; nothing failed.",
                summary.deferred
            )
            .warning()
        );
    }
    Ok(())
}

async fn cmd_coverage(args: AirwayCoverageArgs) -> Result<(), OxyError> {
    let db = connect_db().await?;
    // Single-tenant local: LOCAL_WORKSPACE_ID (Uuid::nil()), matching the
    // `airway_context` platform that cmd_backfill drives with.
    let report = load_coverage(&db, Uuid::nil(), &args.pipeline_ref)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("coverage: {e}")))?;

    if args.json {
        println!("{}", json!(report));
        return Ok(());
    }
    if report.chunks.is_empty() {
        println!("no backfill checkpoints for `{}`", args.pipeline_ref);
        return Ok(());
    }
    println!(
        "{}",
        format!(
            "Coverage {} — {}/{} chunks done",
            args.pipeline_ref, report.summary.done, report.summary.total
        )
        .info()
    );
    if let (Some(f), Some(t)) = (report.summary.loaded_from, report.summary.loaded_to) {
        println!("  loaded range: {} → {}", f.date_naive(), t.date_naive());
    }
    let missing: Vec<_> = report
        .chunks
        .iter()
        .filter(|r| r.status != "done")
        .collect();
    if missing.is_empty() {
        println!("  {}", "no missing periods.".success());
    } else {
        println!("  {} period(s) not done:", missing.len());
        for r in missing {
            let note = r
                .error
                .as_deref()
                .map(|e| format!(": {e}"))
                .unwrap_or_default();
            let mark = match r.status.as_str() {
                "failed" | "cancelled" | "timed_out" => "✗",
                "completed_with_errors" => "⚠",
                _ => "•", // pending / running
            };
            println!(
                "    {mark} {} → {} [{}] (attempts {}){note}",
                r.period_start.date_naive(),
                r.period_end.date_naive(),
                r.status,
                r.attempts,
            );
        }
    }
    Ok(())
}

fn emit(as_json: bool, seq: i64, event_type: &str, payload: &Value) {
    if as_json {
        println!(
            "{}",
            json!({ "seq": seq, "event_type": event_type, "payload": payload })
        );
        return;
    }
    match event_type {
        "load_started" => println!("{}", "▶ load started".info()),
        "extract_completed" => {
            let table = payload.get("table").and_then(Value::as_str).unwrap_or("?");
            let rows = payload
                .get("rows_extracted")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            println!("  extracted {table} ({rows} rows)");
        }
        "normalize_completed" => {
            let table = payload.get("table").and_then(Value::as_str).unwrap_or("?");
            println!("  normalized {table}");
        }
        "destination_load_started" => println!("  loading into destination…"),
        "load_completed" => {
            let ms = payload
                .get("duration_ms")
                .and_then(Value::as_i64)
                .unwrap_or(0);
            println!("{}", format!("✓ load completed ({ms} ms)").success());
        }
        "schema_evolved" => println!("{}", "• schema evolved".secondary()),
        "pipeline_error" => {
            let err = payload.get("error").and_then(Value::as_str).unwrap_or("?");
            println!("{}", format!("✗ pipeline error: {err}").error());
        }
        "cancelled" => println!("{}", "■ cancelled".secondary()),
        other => println!("  {other}: {payload}"),
    }
}

// ── leases ─────────────────────────────────────────────────────────────────

/// `oxy airway leases` — what is currently holding each pipeline.
///
/// Defaults to the local workspace (`Uuid::nil()`); `--workspace-id` selects a
/// cloud one.
async fn cmd_leases(args: AirwayLeasesArgs) -> Result<(), OxyError> {
    let db = connect_db().await?;
    let workspace_id = args.workspace_id.unwrap_or_else(Uuid::nil);
    let leases = agentic_pipeline::airway_run::list_airway_leases(&db, workspace_id)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("list leases: {e}")))?;
    if leases.is_empty() {
        println!("{}", "No airway leases held.".success());
        return Ok(());
    }
    println!(
        "{:<32}  {:<38}  {:<22}  EXPIRES (UTC)",
        "PIPELINE", "RUN_ID", "ACQUIRED (UTC)"
    );
    let now = chrono::Utc::now();
    for l in leases {
        // Flag lapsed rows: they no longer block anything (the next acquire
        // steals them), so an operator seeing one should not go reaching for
        // release-lease.
        let expiry = if l.expires_at <= now {
            format!("{} (LAPSED)", l.expires_at.format("%Y-%m-%d %H:%M:%S"))
        } else {
            l.expires_at.format("%Y-%m-%d %H:%M:%S").to_string()
        };
        println!(
            "{:<32}  {:<38}  {:<22}  {}",
            l.pipeline_name,
            l.run_id,
            l.acquired_at.format("%Y-%m-%d %H:%M:%S"),
            expiry
        );
    }
    Ok(())
}

/// `oxy airway release-lease <pipeline_name>` — recover a stuck pipeline.
///
/// Shows the holder and requires confirmation by default. Releasing a lease
/// whose run is genuinely still executing re-admits exactly the concurrency the
/// lease exists to prevent, so the prompt states that rather than assuming the
/// operator has read the docs.
async fn cmd_release_lease(args: AirwayReleaseLeaseArgs) -> Result<(), OxyError> {
    let db = connect_db().await?;
    let workspace_id = args.workspace_id.unwrap_or_else(Uuid::nil);
    let leases = agentic_pipeline::airway_run::list_airway_leases(&db, workspace_id)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("list leases: {e}")))?;
    let Some(held) = leases
        .iter()
        .find(|l| l.pipeline_name == args.pipeline_name)
    else {
        // Not an error: "already free" is the state the caller wanted.
        println!(
            "{}",
            format!(
                "No lease held for `{}` — nothing to release.",
                args.pipeline_name
            )
            .success()
        );
        return Ok(());
    };

    if !args.force {
        println!(
            "Lease on `{}` is held by run {} (acquired {} UTC, expires {} UTC).",
            held.pipeline_name,
            held.run_id,
            held.acquired_at.format("%Y-%m-%d %H:%M:%S"),
            held.expires_at.format("%Y-%m-%d %H:%M:%S"),
        );
        println!(
            "{}",
            "If that run is still executing, releasing lets a second run start \
             alongside it — which can produce duplicate rows. Only release a \
             holder you know is dead (dead-lettered, or a Ctrl-C'd CLI run)."
                .warning()
        );
        // Decide interactivity BEFORE prompting. `read_line` returns `Ok(0)`
        // for EOF, and EOF on a TTY is Ctrl-D — an operator who reads the
        // warning, decides not to risk it and hits Ctrl-D would otherwise be
        // told their stdin is not interactive and pointed at `--force`: the
        // opposite of what they just decided, in the one message they read
        // while a pipeline is blocked. Checking here also avoids printing a
        // prompt and a two-line warning that nothing will ever read.
        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
            return Err(OxyError::RuntimeError(format!(
                "release-lease needs a confirmation but stdin is not interactive. \
                 Re-run with --force once you have established that run {} is dead.",
                held.run_id
            )));
        }
        print!("Release it? [y/N] ");
        use std::io::Write as _;
        std::io::stdout().flush().ok();
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .map_err(|e| OxyError::RuntimeError(format!("read confirmation: {e}")))?;
        // `Ok(0)` here is Ctrl-D at a real terminal — a deliberate decline,
        // handled by the arm below. The non-interactive case already returned.
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            // A declined confirmation is a non-zero exit, as in git/apt: the
            // lease is still held, so a caller chaining on success must not
            // proceed.
            return Err(OxyError::RuntimeError(
                "aborted; lease left in place".to_string(),
            ));
        }
    }

    // Two deletes, deliberately. The CONFIRMED path is scoped to the run_id the
    // operator was just shown: the prompt waits on human latency while airway
    // pipelines are cron-driven, so an unguarded pipeline-scoped delete lets a
    // `y` typed after the holder finished remove whichever run the next tick
    // started — re-admitting the exact concurrency this table prevents, via the
    // cleanup for it. `--force` skips the prompt, so there is no window and no
    // run to scope to.
    //
    // `workspace_id`, NOT `Uuid::nil()`: the nil literal here meant the flag was
    // honoured for the listing and the prompt above and then discarded for the
    // DELETE, so this could only ever clear a lease in the nil (legacy-local)
    // workspace — while its own help text says the flag is required for a cloud
    // one. It reported success either way.
    let removed = if args.force {
        agentic_pipeline::airway_run::force_release_airway_lease(
            &db,
            workspace_id,
            &args.pipeline_name,
        )
        .await
    } else {
        agentic_pipeline::airway_run::release_airway_lease_scoped(
            &db,
            workspace_id,
            &args.pipeline_name,
            &held.run_id,
        )
        .await
    }
    .map_err(|e| OxyError::RuntimeError(format!("release lease: {e}")))?;

    if removed == 0 {
        // Zero rows means two different things now that the confirmed delete is
        // run-scoped, and an operator acts differently on each: either the
        // holder released on its own and NOBODY took over (the pipeline is
        // free — the same end state the early return above reports as success),
        // or a successor claimed it (still blocked). Erroring on both would
        // abort `release-lease … && restart-something` on the benign half — and
        // the whole argument for run-scoping was that this window is
        // human-scale, which makes that half exactly as reachable as the one it
        // fixed. So ask who holds it now.
        let holder_now = agentic_pipeline::airway_run::list_airway_leases(&db, workspace_id)
            .await
            .map_err(|e| OxyError::RuntimeError(format!("list leases: {e}")))?
            .into_iter()
            .find(|l| l.pipeline_name == args.pipeline_name);
        match holder_now {
            None => {
                println!(
                    "{}",
                    format!(
                        "Lease on `{}` was already released by run {}; the pipeline can run again.",
                        args.pipeline_name, held.run_id
                    )
                    .success()
                );
                return Ok(());
            }
            Some(next) => {
                // The successor can be the SAME run: `retry` re-acquires under
                // the original run_id on purpose (see `retry.rs`), so a holder
                // that terminalized, released, and was then retried shows up
                // here with an unchanged id. Without this arm the operator
                // reads "run X no longer holds it, run X does now" — the one
                // wording that leaves them nothing to act on.
                if next.run_id == held.run_id {
                    // Echo the flag only if the operator actually passed one —
                    // `workspace_id` defaults to `Uuid::nil()`, and suggesting
                    // `--workspace-id 00000000-…` hands a legacy-local operator
                    // a UUID they never typed, in the one message they read
                    // while a pipeline is blocked.
                    let ws_flag = match args.workspace_id {
                        Some(id) => format!(" --workspace-id {id}"),
                        None => String::new(),
                    };
                    return Err(OxyError::RuntimeError(format!(
                        "lease on `{}` was not released: run {} released it and was \
                         retried, so it holds the lease again and is live. Leave it \
                         alone, or re-check with `oxy airway leases{ws_flag}`.",
                        args.pipeline_name, held.run_id
                    )));
                }
                // Still blocked, by a DIFFERENT run than the operator confirmed
                // against — never silently widen to it.
                //
                // Both arms below describe the same end state, so they must not
                // give opposite advice. Pointing the confirmed path at `--force`
                // would tell the operator to run a pipeline-scoped delete against
                // a successor they have never been shown — the exact hazard
                // run-scoping closed, reached one command later instead of
                // through a `y`. Re-running the plain command is strictly better:
                // it re-lists on entry, so it shows the new holder with its
                // acquire/expiry times, prompts fresh, and deletes scoped to that
                // run. The operator decides against the run they would actually
                // free. `--force` stays for when they have already established
                // the successor is dead.
                return Err(OxyError::RuntimeError(if args.force {
                    format!(
                        "lease on `{}` was not released: run {} was already gone, \
                         and run {} acquired it since. That run is live — leave it alone.",
                        args.pipeline_name, held.run_id, next.run_id
                    )
                } else {
                    format!(
                        "lease on `{}` was not released: run {} no longer holds it, \
                         run {} does now. Re-run the same command to review and \
                         confirm against that run.",
                        args.pipeline_name, held.run_id, next.run_id
                    )
                }));
            }
        }
    }
    println!(
        "{}",
        format!(
            "Released {removed} lease(s) for `{}`; the pipeline can run again.",
            args.pipeline_name
        )
        .success()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every status that stops the poll loop must also have an exit code.
    ///
    /// These two lists are what `oxy airway run` reports with, and they drift
    /// in opposite directions: a status missing from `TERMINAL_STATUSES` hangs
    /// the CLI until `--wait` expires on a run that already finished, and one
    /// missing from `FAILED_STATUSES` exits 0 on a run that landed nothing.
    #[test]
    fn every_failed_status_is_also_terminal() {
        for s in FAILED_STATUSES {
            assert!(
                TERMINAL_STATUSES.contains(s),
                "`{s}` exits non-zero but never ends the poll loop"
            );
        }
    }

    #[test]
    fn a_lost_pipeline_exits_non_zero_and_a_clean_one_does_not() {
        for s in ["failed", "cancelled", "timed_out"] {
            assert!(is_terminal(Some(s)), "`{s}` must end the wait");
            assert!(FAILED_STATUSES.contains(&s), "`{s}` must exit non-zero");
        }
        assert!(is_terminal(Some("done")));
        assert!(
            !FAILED_STATUSES.contains(&"done"),
            "a clean run must exit 0"
        );
    }

    /// A run still in flight — or one whose status we have never seen — must
    /// keep the CLI waiting rather than being reported as some outcome.
    #[test]
    fn a_running_or_unknown_status_is_not_terminal() {
        for s in [
            None,
            Some("running"),
            Some("delegating"),
            Some("awaiting_input"),
            Some(""),
        ] {
            assert!(!is_terminal(s), "{s:?} must not end the wait");
        }
    }
}
