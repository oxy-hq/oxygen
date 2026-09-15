//! `oxy agentic` CLI — run and debug agentic pipelines.
//!
//! Shares the same domain logic as the HTTP endpoints via `agentic-pipeline`.
//! Supports dual-mode output: rich terminal (default) or JSONL (`--json`).

use std::sync::Arc;

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

use agentic_pipeline::PipelineBuilder;
use agentic_pipeline::{AnalyticsMigrator, AutomationMigrator};
use agentic_runtime::crud;
use agentic_runtime::crud::user_facing_status;
use agentic_runtime::event_registry::EventRegistry;
use agentic_runtime::migration::RuntimeMigrator;
use agentic_runtime::state::RuntimeState;

// ── Args ────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
pub struct AgenticArgs {
    #[clap(subcommand)]
    pub command: AgenticCommand,
}

#[derive(Parser, Debug)]
pub enum AgenticCommand {
    /// Run an agentic pipeline interactively
    Run(RunArgs),
    /// List recent pipeline runs
    List(ListArgs),
    /// Replay events for a run
    Events(EventsArgs),
    /// Submit an answer to a suspended run
    Answer(AnswerArgs),
    /// Cancel a running pipeline
    Cancel(CancelArgs),
    /// Inspect a run's details
    Inspect(InspectArgs),
    /// Show live status of active runs, tasks, and delegations
    Status(StatusArgs),
}

#[derive(Parser, Debug)]
pub struct RunArgs {
    /// Agent config name (e.g. `queries_agent` loads `queries_agent.agentic.yml`)
    pub config: String,
    /// Question to ask the pipeline
    #[clap(long, short)]
    pub question: String,
    /// Domain: "analytics" (default) or "builder"
    #[clap(long, default_value = "analytics")]
    pub domain: String,
    /// Thinking mode: "auto" (default) or "extended_thinking"
    #[clap(long, default_value = "auto")]
    pub thinking_mode: String,
    /// Output structured JSONL instead of pretty terminal
    #[clap(long)]
    pub json: bool,
    /// Workspace to run as. Defaults to the local workspace (`Uuid::nil()`).
    ///
    /// Needed for anything that resolves per tenant — a `postgres_managed`
    /// database maps workspace → org → `oltp_tenants`, and the nil workspace
    /// belongs to no org. Matches `oxy airway run --workspace-id`.
    #[clap(long)]
    pub workspace_id: Option<Uuid>,
}

#[derive(Parser, Debug)]
pub struct ListArgs {
    /// Maximum number of runs to show
    #[clap(long, default_value = "20")]
    pub limit: u64,
    /// Output JSON
    #[clap(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct EventsArgs {
    /// Run ID to replay events for
    pub run_id: String,
    /// Start after this sequence number
    #[clap(long, default_value = "-1")]
    pub after_seq: i64,
    /// Output JSONL
    #[clap(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct AnswerArgs {
    pub run_id: String,
    /// The answer text
    #[clap(long, short)]
    pub answer: String,
}

#[derive(Parser, Debug)]
pub struct CancelArgs {
    pub run_id: String,
}

#[derive(Parser, Debug)]
pub struct InspectArgs {
    pub run_id: String,
    /// Output JSON
    #[clap(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct StatusArgs {
    /// Watch mode: poll every N seconds (0 = single shot)
    #[clap(long, short, default_value = "0")]
    pub watch: u64,
    /// Only show active (running/suspended) runs
    #[clap(long)]
    pub active: bool,
    /// Output JSON
    #[clap(long)]
    pub json: bool,
}

// ── Dispatch ────────────────────────────────────────────────────────────────

pub async fn handle_agentic_command(args: AgenticArgs) -> Result<(), OxyError> {
    match args.command {
        AgenticCommand::Run(a) => cmd_run(a).await,
        AgenticCommand::List(a) => cmd_list(a).await,
        AgenticCommand::Events(a) => cmd_events(a).await,
        AgenticCommand::Answer(a) => cmd_answer(a).await,
        AgenticCommand::Cancel(a) => cmd_cancel(a).await,
        AgenticCommand::Inspect(a) => cmd_inspect(a).await,
        AgenticCommand::Status(a) => cmd_status(a).await,
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

async fn connect_db() -> Result<DatabaseConnection, OxyError> {
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
    Ok(db)
}

fn build_event_registry() -> EventRegistry {
    let mut registry = EventRegistry::new();
    registry.register("analytics", agentic_analytics::event_handler());
    registry.register("builder", agentic_builder::event_handler());
    registry
}

// ── Commands ────────────────────────────────────────────────────────────────

async fn cmd_run(args: RunArgs) -> Result<(), OxyError> {
    let db = connect_db().await?;
    let project_path = resolve_local_workspace_path()?;
    // main's `--workspace-id` (falling back to nil) with this branch's builder
    // terminal: `with_workspace_path` is gone, and `with_working_copy` is the
    // door that states what it needs — a root, an optional pinned revision, and
    // what to do when `config.yml` is absent. `Fail` here because a CLI run
    // against a project without one has nothing to do.
    let workspace_manager = WorkspaceBuilder::new(args.workspace_id.unwrap_or_else(Uuid::nil))
        .with_working_copy(&project_path, None, oxy::config::OnMissing::Fail)
        .await?
        .build()
        .await?;

    let thinking_mode = match args.thinking_mode.as_str() {
        "extended_thinking" => agentic_pipeline::ThinkingMode::ExtendedThinking,
        _ => agentic_pipeline::ThinkingMode::Auto,
    };

    let project_ctx = Arc::new(crate::agentic_wiring::OxyProjectContext::new(
        workspace_manager,
    ));
    let platform: Arc<dyn agentic_pipeline::platform::PlatformContext> = project_ctx.clone();
    let bridges = crate::agentic_wiring::build_builder_bridges(project_ctx);
    let platform_for_delegation = platform.clone();
    let bridges_for_delegation = bridges.clone();
    let workspace_id = platform.workspace_id();
    let mut builder = PipelineBuilder::new(platform)
        .workspace_id(workspace_id)
        .with_builder_bridges(bridges)
        .question(&args.question)
        .thinking_mode(thinking_mode);

    builder = if args.domain == "builder" {
        builder.builder(None)
    } else {
        builder.analytics(&args.config)
    };

    let started = builder
        .start(&db)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("pipeline start failed: {e}")))?;

    let run_id = started.run_id.clone();
    let source_type = started.source_type.clone();

    if !args.json {
        println!("{}", format!("Run started: {run_id}").info());
    }

    // Set up channels and state for driving the pipeline.
    let state = Arc::new(RuntimeState::new());
    let (answer_tx, answer_rx) = mpsc::channel::<String>(1);
    let (cancel_tx, cancel_rx) = watch::channel(false);
    state.register(&run_id, answer_tx, cancel_tx);

    let db2 = db.clone();
    let state2 = Arc::clone(&state);
    let drive_handle = tokio::spawn(async move {
        agentic_pipeline::drive_with_coordinator(
            started,
            db2,
            state2,
            answer_rx,
            cancel_rx,
            platform_for_delegation,
            Some(bridges_for_delegation),
            None,
            None,
            None,
            // CLI is one-shot: no long-lived listener to maintain, so
            // the no-op router is correct. Worker falls back to the 10s
            // backstop poll, which matches the CLI's typical runtime
            // (single automation, no cross-process coordination).
            Arc::new(agentic_runtime::router::NoopTaskRouter),
        )
        .await;
    });

    // Stream events to terminal as they arrive.
    let registry = build_event_registry();
    let mut processor = registry.stream_processor(&source_type);
    let mut last_seq: i64 = -1;

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let rows = crud::get_events_after(&db, &run_id, last_seq)
            .await
            .unwrap_or_default();

        let mut terminal = false;
        for row in rows {
            last_seq = row.seq;
            let ui_events = processor.process(&row.event_type, &row.payload);
            for (event_type, payload) in ui_events {
                if args.json {
                    println!(
                        "{}",
                        json!({ "seq": row.seq, "event_type": event_type, "payload": payload })
                    );
                } else {
                    render_event_pretty(&event_type, &payload);
                }
                if event_type == "done" || event_type == "error" {
                    terminal = true;
                }
            }
        }

        if terminal {
            break;
        }

        // Check if pipeline task has finished.
        if drive_handle.is_finished() {
            // Final sweep.
            let rows = crud::get_events_after(&db, &run_id, last_seq)
                .await
                .unwrap_or_default();
            for row in rows {
                let ui_events = processor.process(&row.event_type, &row.payload);
                for (event_type, payload) in ui_events {
                    if args.json {
                        println!(
                            "{}",
                            json!({ "seq": row.seq, "event_type": event_type, "payload": payload })
                        );
                    } else {
                        render_event_pretty(&event_type, &payload);
                    }
                }
            }
            break;
        }
    }

    drive_handle.await.ok();
    Ok(())
}

async fn cmd_list(args: ListArgs) -> Result<(), OxyError> {
    let db = connect_db().await?;

    // List recent runs (all runs, not just by thread). CLI is local-mode
    // only, so the workspace id is fixed to `LOCAL_WORKSPACE_ID` —
    // matches what `start_automation_run` / `start_airway_run` seed with
    // from the CLI entry points.
    let runs = crud::list_recent_runs(
        &db,
        oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID,
        args.limit,
    )
    .await
    .map_err(|e| OxyError::RuntimeError(format!("db error: {e}")))?;

    if args.json {
        let items: Vec<Value> = runs
            .iter()
            .map(|r| {
                json!({
                    "run_id": r.id,
                    "status": user_facing_status(r.task_status.as_deref()),
                    "source_type": r.source_type,
                    "question": r.question,
                    "created_at": r.created_at.to_string(),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items).unwrap());
    } else {
        if runs.is_empty() {
            println!("No runs found.");
            return Ok(());
        }
        println!(
            "{:<12} {:<10} {:<10} QUESTION",
            "RUN ID", "STATUS", "DOMAIN"
        );
        println!("{}", "─".repeat(70));
        for r in &runs {
            let short_id = if r.id.len() > 10 { &r.id[..10] } else { &r.id };
            let source = r.source_type.as_deref().unwrap_or("?");
            let q = if r.question.len() > 40 {
                format!("{}...", &r.question[..37])
            } else {
                r.question.clone()
            };
            let status = user_facing_status(r.task_status.as_deref());
            let status_colored = match status {
                "done" => status.success(),
                "failed" => status.error(),
                "running" => status.info(),
                "suspended" => status.warning(),
                _ => status.text(),
            };
            println!(
                "{:<12} {:<10} {:<10} {}",
                short_id, status_colored, source, q
            );
        }
    }
    Ok(())
}

async fn cmd_events(args: EventsArgs) -> Result<(), OxyError> {
    let db = connect_db().await?;
    let registry = build_event_registry();

    let source_type = crud::get_run(&db, &args.run_id)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("db error: {e}")))?
        .map(|r| r.source_type.unwrap_or_else(|| "analytics".to_string()))
        .unwrap_or_else(|| "analytics".to_string());

    let mut processor = registry.stream_processor(&source_type);
    let rows = crud::get_events_after(&db, &args.run_id, args.after_seq)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("db error: {e}")))?;

    if rows.is_empty() {
        if !args.json {
            println!("No events found for run {}.", args.run_id);
        }
        return Ok(());
    }

    for row in rows {
        let ui_events = processor.process(&row.event_type, &row.payload);
        for (event_type, payload) in ui_events {
            if args.json {
                println!(
                    "{}",
                    json!({ "seq": row.seq, "event_type": event_type, "payload": payload })
                );
            } else {
                render_event_pretty(&event_type, &payload);
            }
        }
    }
    Ok(())
}

async fn cmd_answer(args: AnswerArgs) -> Result<(), OxyError> {
    let db = connect_db().await?;
    // CLI answer: update the run status directly (the pipeline may be running
    // in the HTTP server, not in this process).
    crud::update_run_running(&db, &args.run_id)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("db error: {e}")))?;
    println!(
        "{}",
        format!("Answer submitted for run {}.", args.run_id).success()
    );
    Ok(())
}

async fn cmd_cancel(args: CancelArgs) -> Result<(), OxyError> {
    let db = connect_db().await?;
    crud::update_run_failed(&db, &args.run_id, "cancelled by user")
        .await
        .map_err(|e| OxyError::RuntimeError(format!("db error: {e}")))?;
    println!("{}", format!("Run {} cancelled.", args.run_id).success());
    Ok(())
}

async fn cmd_inspect(args: InspectArgs) -> Result<(), OxyError> {
    let db = connect_db().await?;

    let run = crud::get_run(&db, &args.run_id)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("db error: {e}")))?
        .ok_or_else(|| OxyError::RuntimeError(format!("run {} not found", args.run_id)))?;

    let ext = agentic_analytics::get_run_meta(&db, &args.run_id)
        .await
        .ok()
        .flatten();

    let event_count = crud::get_max_seq(&db, &args.run_id).await.unwrap_or(-1) + 1;

    let run_status = user_facing_status(run.task_status.as_deref());

    if args.json {
        let mut obj = json!({
            "run_id": run.id,
            "status": run_status,
            "source_type": run.source_type,
            "question": run.question,
            "answer": run.answer,
            "error_message": run.error_message,
            "metadata": run.metadata,
            "created_at": run.created_at.to_string(),
            "updated_at": run.updated_at.to_string(),
            "event_count": event_count,
        });
        if let Some(e) = &ext {
            obj["extension"] = json!({
                "agent_id": e.agent_id,
                "thinking_mode": e.thinking_mode,
                "has_spec_hint": e.spec_hint.is_some(),
            });
        }
        println!("{}", serde_json::to_string_pretty(&obj).unwrap());
    } else {
        println!("{}", "Run Details".text());
        println!("{}", "─".repeat(50));
        println!("  Run ID:      {}", run.id);
        println!(
            "  Status:      {}",
            match run_status {
                "done" => run_status.success(),
                "failed" => run_status.error(),
                "running" => run_status.info(),
                "suspended" => run_status.warning(),
                _ => run_status.text(),
            }
        );
        println!(
            "  Domain:      {}",
            run.source_type.as_deref().unwrap_or("?")
        );
        println!("  Question:    {}", run.question);
        if let Some(answer) = &run.answer {
            println!(
                "  Answer:      {}",
                if answer.len() > 80 {
                    format!("{}...", &answer[..77])
                } else {
                    answer.clone()
                }
            );
        }
        if let Some(err) = &run.error_message {
            println!("  Error:       {}", err.error());
        }
        println!("  Events:      {}", event_count);
        println!("  Created:     {}", run.created_at);
        if let Some(e) = &ext {
            println!();
            println!("{}", "Analytics Extension".text());
            println!("{}", "─".repeat(50));
            println!("  Agent ID:    {}", e.agent_id);
            if let Some(tm) = &e.thinking_mode {
                println!("  Thinking:    {}", tm);
            }
            if e.spec_hint.is_some() {
                println!("  Spec Hint:   present");
            }
        }
    }
    Ok(())
}

// ── Pretty renderer ─────────────────────────────────────────────────────────

fn render_event_pretty(event_type: &str, payload: &Value) {
    match event_type {
        "step_start" => {
            let label = payload["label"].as_str().unwrap_or("?");
            let summary = payload["summary"].as_str().unwrap_or("");
            if summary.is_empty() {
                println!("⠋ {}", label.info());
            } else {
                println!("⠋ {} — {}", label.info(), summary);
            }
        }
        "step_end" => {
            let label = payload["label"].as_str().unwrap_or("?");
            let outcome = payload["outcome"].as_str().unwrap_or("?");
            let marker = if outcome == "advanced" { "✓" } else { "✗" };
            let colored = if outcome == "advanced" {
                format!("{marker} {label}").success()
            } else {
                format!("{marker} {label}").warning()
            };
            println!("{colored}");
        }
        "tool_call" => {
            let name = payload["name"].as_str().unwrap_or("?");
            let input = payload["input"].as_str().unwrap_or("");
            let truncated = if input.len() > 60 {
                format!("{}...", &input[..57])
            } else {
                input.to_string()
            };
            println!("  → {}({})", name.secondary(), truncated);
        }
        "tool_result" => {
            let name = payload["name"].as_str().unwrap_or("?");
            let output = payload["output"].as_str().unwrap_or("");
            let truncated = if output.len() > 60 {
                format!("{}...", &output[..57])
            } else {
                output.to_string()
            };
            println!("  ← {} → {}", name.secondary(), truncated);
        }
        "text_delta" => {
            let token = payload["token"].as_str().unwrap_or("");
            print!("{token}");
            use std::io::Write;
            std::io::stdout().flush().ok();
        }
        "thinking_token" => {
            // Dimmed thinking tokens — skip for now in pretty mode.
        }
        "done" => {
            println!();
            println!("{}", "✓ Done".success());
        }
        "error" => {
            let msg = payload["message"].as_str().unwrap_or("unknown error");
            println!();
            println!("{}", format!("✗ Error: {msg}").error());
        }
        "awaiting_input" => {
            println!();
            if let Some(questions) = payload["questions"].as_array() {
                for q in questions {
                    let prompt = q["prompt"].as_str().unwrap_or("?");
                    println!("{}", format!("? {prompt}").warning());
                }
            }
            println!("  (Use `oxy agentic answer <run_id> --answer \"...\"` to respond)");
        }
        "tool_used" => {
            let name = payload["tool_name"].as_str().unwrap_or("?");
            let summary = payload["summary"].as_str().unwrap_or("");
            println!("  🔧 {} — {}", name.secondary(), summary);
        }
        "file_change_pending" => {
            let path = payload["file_path"].as_str().unwrap_or("?");
            let desc = payload["description"].as_str().unwrap_or("");
            println!("  📝 {} — {}", path.info(), desc);
        }
        _ => {
            // Other events: show type in dimmed text.
            println!("  [{event_type}]");
        }
    }
}

// ── Status command ──────────────────────────────────────────────────────────

async fn cmd_status(args: StatusArgs) -> Result<(), OxyError> {
    let db = connect_db().await?;

    loop {
        let runs = if args.active {
            get_active_runs(&db).await?
        } else {
            crud::list_recent_runs(&db, oxy_app_core::serve_mode::LOCAL_WORKSPACE_ID, 20)
                .await
                .map_err(db_err)?
        };

        if args.json {
            print_status_json(&db, &runs).await;
        } else {
            print_status_table(&db, &runs).await;
        }

        if args.watch == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_secs(args.watch)).await;
        // Clear screen for watch mode.
        print!("\x1B[2J\x1B[1;1H");
    }

    Ok(())
}

async fn get_active_runs(
    db: &DatabaseConnection,
) -> Result<Vec<agentic_runtime::entity::run::Model>, OxyError> {
    use agentic_runtime::entity::run;
    use sea_orm::{ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder};

    run::Entity::find()
        .filter(
            Condition::any()
                // Active task states.
                .add(run::Column::TaskStatus.eq("running"))
                .add(run::Column::TaskStatus.eq("awaiting_input"))
                .add(run::Column::TaskStatus.eq("delegating"))
                // Recovery states that indicate pending work.
                .add(run::Column::TaskStatus.eq("needs_resume"))
                .add(run::Column::TaskStatus.eq("shutdown")),
        )
        .order_by_desc(run::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err)
}

async fn print_status_table(db: &DatabaseConnection, runs: &[agentic_runtime::entity::run::Model]) {
    if runs.is_empty() {
        println!("{}", "No runs found.".tertiary());
        return;
    }

    println!(
        "{:<38} {:<12} {:<16} {:<12} {:<8} {:<20} {}",
        "RUN ID".text(),
        "STATUS".text(),
        "TASK STATUS".text(),
        "DOMAIN".text(),
        "EVENTS".text(),
        "CURRENT STAGE".text(),
        "DELEGATIONS".text(),
    );
    println!("{}", "─".repeat(130));

    for run in runs {
        let event_count = crud::get_max_seq(db, &run.id).await.unwrap_or(-1) + 1;

        // Find current stage from last state_enter event.
        let last_stage = get_last_stage(db, &run.id).await;

        // Count delegation events.
        let delegation_info = get_delegation_info(db, &run.id).await;

        let status = user_facing_status(run.task_status.as_deref());
        let status_display = match status {
            "done" => status.success(),
            "failed" => status.error(),
            "running" => status.info(),
            "suspended" => status.warning(),
            _ => status.text(),
        };

        let task_status_display = match run.task_status.as_deref() {
            Some("needs_resume") => "needs_resume".warning(),
            Some("shutdown") => "shutdown".warning(),
            Some("done") => "done".success(),
            Some("failed") => "failed".error(),
            Some(s) => s.text(),
            None => "-".tertiary(),
        };

        println!(
            "{:<38} {:<12} {:<16} {:<12} {:<8} {:<20} {}",
            run.id,
            status_display,
            task_status_display,
            run.source_type.as_deref().unwrap_or("?"),
            event_count,
            last_stage,
            delegation_info,
        );
    }
}

async fn print_status_json(db: &DatabaseConnection, runs: &[agentic_runtime::entity::run::Model]) {
    let mut results = Vec::new();
    for run in runs {
        let event_count = crud::get_max_seq(db, &run.id).await.unwrap_or(-1) + 1;
        let last_stage = get_last_stage(db, &run.id).await;
        let delegation_info = get_delegation_info(db, &run.id).await;

        results.push(json!({
            "run_id": run.id,
            "status": user_facing_status(run.task_status.as_deref()),
            "source_type": run.source_type,
            "question": run.question,
            "event_count": event_count,
            "current_stage": last_stage,
            "delegations": delegation_info,
            "created_at": run.created_at.to_string(),
        }));
    }
    println!("{}", serde_json::to_string_pretty(&results).unwrap());
}

async fn get_last_stage(db: &DatabaseConnection, run_id: &str) -> String {
    let events = crud::get_all_events(db, run_id).await.unwrap_or_default();

    // Walk events in reverse to find the last state_enter.
    for event in events.iter().rev() {
        if event.event_type == "state_enter"
            && let Some(label) = event.payload["state"].as_str()
        {
            return label.to_string();
        }
    }
    "—".to_string()
}

async fn get_delegation_info(db: &DatabaseConnection, run_id: &str) -> String {
    let events = crud::get_all_events(db, run_id).await.unwrap_or_default();

    let mut started = 0u32;
    let mut completed = 0u32;
    let mut active_target = None;

    for event in &events {
        match event.event_type.as_str() {
            "delegation_started" => {
                started += 1;
                active_target = event.payload["target"].as_str().map(String::from);
            }
            "delegation_completed" => {
                completed += 1;
                active_target = None;
            }
            _ => {}
        }
    }

    if started == 0 {
        "none".to_string()
    } else if let Some(target) = active_target {
        format!("{started} started, {completed} done, active: {target}")
    } else {
        format!("{started} started, {completed} done")
    }
}

fn db_err(e: sea_orm::DbErr) -> OxyError {
    OxyError::RuntimeError(format!("db error: {e}"))
}
