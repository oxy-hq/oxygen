//! Operator-only `oxy admin …` subcommands.
//!
//! Today this hosts the airhouse SA rotation flow used as a bearer-leak
//! response. Future operator commands (e.g. forced re-provision, SA
//! audits) belong here too — anything that mutates deployment-wide state
//! and shouldn't be reachable through the user-facing surfaces.

use std::sync::Arc;

use agentic_pipeline::airway_run::{StartAirwayRequest, start_airway_run};
use agentic_pipeline::automation_run::{StartAutomationRequest, start_automation_run};
use agentic_pipeline::{AirwayMigrator, AnalyticsMigrator, AutomationMigrator};
use agentic_runtime::migration::RuntimeMigrator;
use clap::Parser;
use migration::MigratorTrait;
use oxy::adapters::workspace::builder::WorkspaceBuilder;
use oxy::config::resolve_local_workspace_path;
use oxy::database::client::establish_connection;
use oxy::theme::StyledText;
use oxy_shared::errors::OxyError;
use uuid::Uuid;

#[derive(Parser, Debug)]
pub struct AdminArgs {
    #[clap(subcommand)]
    pub command: AdminCommand,
}

#[derive(Parser, Debug)]
pub enum AdminCommand {
    /// Airhouse integration administration (SA rotation, …).
    Airhouse(AirhouseArgs),
    /// Seed a single Global (`scope_owned = false`) run and exit.
    ///
    /// Operator/soak tool only — NOT the Phase 2 scheduler. Inserts the
    /// run + a `scope_owned = false` queued task and returns immediately
    /// WITHOUT spawning a coordinator, so the run is driven solely by the
    /// standalone global driver loop (enable `OXY_INPROC_GLOBAL_WORKER`).
    /// Use this to exercise the Phase 1 consumer in a staging soak.
    /// Requires `OXY_DATABASE_URL`.
    SeedGlobalRun(SeedGlobalRunArgs),
}

#[derive(Parser, Debug)]
pub struct SeedGlobalRunArgs {
    /// Seed a workflow run: path to a `.automation.yml` /
    /// `.procedure.yml`, relative to the workspace root.
    #[clap(long, value_name = "REF", conflicts_with = "airway")]
    pub workflow: Option<String>,
    /// Seed an airway run: path to a `.airway.yml`, relative to the
    /// workspace root.
    #[clap(long, value_name = "REF")]
    pub airway: Option<String>,
}

#[derive(Parser, Debug)]
pub struct AirhouseArgs {
    #[clap(subcommand)]
    pub command: AirhouseCommand,
}

#[derive(Parser, Debug)]
pub enum AirhouseCommand {
    /// Rotate the per-tenant Airhouse service account for a workspace.
    ///
    /// Use this as the bearer-leak response: the old SA is revoked
    /// immediately on the airhouse side, a fresh SA is minted under the
    /// same deterministic name, and its bearer is sealed onto the
    /// `airhouse_tenants` row. Outstanding ephemerals issued by the old
    /// SA continue to authenticate via SCRAM until they expire (24h
    /// max); fresh mints route through the new SA on the next broker
    /// cache miss.
    RotateSa(RotateSaArgs),
}

#[derive(Parser, Debug)]
pub struct RotateSaArgs {
    /// Workspace whose tenant should rotate. Must already be provisioned.
    pub workspace_id: Uuid,
}

/// Dispatch `oxy admin …`.
pub async fn handle_admin_command(args: AdminArgs) -> Result<(), OxyError> {
    match args.command {
        AdminCommand::Airhouse(airhouse_args) => match airhouse_args.command {
            AirhouseCommand::RotateSa(rotate_args) => handle_rotate_sa(rotate_args).await,
        },
        AdminCommand::SeedGlobalRun(args) => handle_seed_global_run(args).await,
    }
}

enum SeedTarget {
    Workflow(String),
    Airway(String),
}

async fn handle_seed_global_run(args: SeedGlobalRunArgs) -> Result<(), OxyError> {
    // Validate the target up front so a usage mistake is a clear message,
    // not a downstream DB-connection error. (`--workflow`+`--airway`
    // together is already rejected by clap's `conflicts_with`.)
    let target = match (args.workflow, args.airway) {
        (Some(w), None) => SeedTarget::Workflow(w),
        (None, Some(a)) => SeedTarget::Airway(a),
        (None, None) => {
            return Err(OxyError::ConfigurationError(
                "pass one of --workflow <ref> or --airway <ref>".into(),
            ));
        }
        (Some(_), Some(_)) => {
            return Err(OxyError::ConfigurationError(
                "pass exactly one of --workflow or --airway".into(),
            ));
        }
    };

    let db = establish_connection().await?;
    // Idempotent against an already-migrated staging DB; also makes the
    // command work against a fresh DB. Mirrors `oxy airway`'s connect path.
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

    let run_id = match target {
        SeedTarget::Workflow(workflow_ref) => {
            let request = StartAutomationRequest {
                workflow_ref,
                variables: None,
                retry_from_run_id: None,
                cache_enabled: false,
                invalidate_steps: None,
                invalidate_iterations: None,
                thread_id: None,
                schedule_id: None,
                trigger: None,
                logical_date: None,
                retry_of: None,
            };
            // Global: no co-located coordinator. The standalone global
            // driver loop (OXY_INPROC_GLOBAL_WORKER) must pick it up.
            // CLI seeds against the local workspace (Uuid::nil() ==
            // LOCAL_WORKSPACE_ID); the global driver loop resolves the
            // matching cached PlatformContext via that key.
            start_automation_run(
                &db,
                request,
                agentic_pipeline::TaskScope::Global,
                Uuid::nil(),
            )
            .await
            .map_err(|e| OxyError::RuntimeError(format!("seed workflow: {e}")))?
        }
        SeedTarget::Airway(pipeline_ref) => {
            // `start_airway_run` needs a workspace surface to resolve the
            // `.airway.yml`; build it from the local workspace like
            // `oxy airway` does.
            let project_path = resolve_local_workspace_path()?;
            let workspace_manager = WorkspaceBuilder::new(Uuid::nil())
                .with_working_copy(&project_path, None, oxy::config::OnMissing::Fail)
                .await?
                .build()
                .await?;
            let project_ctx = Arc::new(crate::agentic_wiring::OxyProjectContext::new(
                workspace_manager,
            ));
            let workspace: Arc<dyn agentic_pipeline::WorkflowWorkspaceContext> = project_ctx;
            let request = StartAirwayRequest {
                pipeline_ref,
                variables: None,
                thread_id: None,
                resources: Vec::new(),
                schedule_id: None,
                trigger: None,
                logical_date: None,
                retry_of: None,
                backfill_from: None,
                backfill_to: None,
            };
            start_airway_run(
                &db,
                workspace.as_ref(),
                request,
                agentic_pipeline::TaskScope::Global,
                Uuid::nil(),
            )
            .await
            .map_err(|e| OxyError::RuntimeError(format!("seed airway: {e}")))?
        }
    };

    println!(
        "{}",
        format!("Seeded Global run {run_id} (scope_owned=false, no driver).").success()
    );
    println!(
        "{}",
        "The periodic global driver loop drives it — on by default for every \
         role except `serve` (force with OXY_INPROC_GLOBAL_WORKER=1)."
            .text()
    );
    Ok(())
}

async fn handle_rotate_sa(args: RotateSaArgs) -> Result<(), OxyError> {
    let db = establish_connection().await?;
    let provisioner = airhouse::provisioner_for(db).ok_or_else(|| {
        OxyError::ConfigurationError(
            "airhouse integration is not configured for this deployment; \
             set AIRHOUSE_BASE_URL / AIRHOUSE_ADMIN_TOKEN / AIRHOUSE_WIRE_HOST / \
             AIRHOUSE_WIRE_PORT before running `oxy admin airhouse rotate-sa`"
                .into(),
        )
    })?;

    println!(
        "{}",
        format!("Rotating airhouse SA for workspace {}…", args.workspace_id).text()
    );
    let rotated = provisioner
        .rotate_service_account(args.workspace_id)
        .await
        .map_err(|e| OxyError::RuntimeError(format!("rotate-sa failed: {e}")))?;

    println!(
        "{}",
        format!(
            "Rotated SA: {} → {} at {}",
            rotated.old_sa_id, rotated.new_sa_id, rotated.rotated_at
        )
        .success()
    );

    // Other replicas keep an in-process broker cache keyed off the old SA
    // bearer; their next mint will get a 401 and fall through to
    // `evict_and_remint`. There is no in-band way to push the rotation to
    // them from this CLI process, so we just exit. Operators who need a
    // hard cutover should restart replicas after rotation.
    Ok(())
}
