//! Operator-only `oxy admin …` subcommands.
//!
//! Today this hosts the airhouse SA rotation flow used as a bearer-leak
//! response. Future operator commands (e.g. forced re-provision, SA
//! audits) belong here too — anything that mutates deployment-wide state
//! and shouldn't be reachable through the user-facing surfaces.

use clap::Parser;
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
    }
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
