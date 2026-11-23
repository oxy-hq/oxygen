//! Minting an `airhouse_managed` credential: whose name goes on it
//! ([`CredentialUse`]) and which broker call makes it. Moved out of
//! `connector.rs`, which kept the callers.

use entity::workspace_members::WorkspaceRole;
use oxy::adapters::workspace::manager::WorkspaceManager;
use oxy_shared::errors::OxyError;

use super::AirhouseWireParams;

/// Whether the credential a mint returns will be **shared** with other
/// requesters or belongs to **one** request. The caller knows; this function
/// cannot tell, and the answer decides whether naming the requesting user on
/// the credential is true or false.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialUse {
    /// Handed to [`airhouse_pool`](crate::airhouse_pool), so
    /// the connection it opens is reused by every other requester at the same
    /// role. Minted for [`airhouse::SystemPurpose::SharedQuery`] — a per-user
    /// mint here would stamp whoever opened the connection onto queries other
    /// people ran.
    SharedPool,
    /// A DSN for one consumer that opens its own connection and pools nothing
    /// (the airway destination resolver,
    /// `project_context::resolve_pipeline_destination`). Nobody else uses it,
    /// so the requester is minted by name and the audit row is exact.
    PerRequest,
    /// A react run's pooled query connection: a Reader whose credential
    /// Airhouse confirmed may only read (`AirhouseTokenBroker::mint_read_only`,
    /// which refuses one without the `read_only` echo). Minted for the
    /// workspace, not a person, and the same for every requester: a Reader
    /// grants no more than any member's role.
    ReadOnly,
}

/// Mint an ephemeral wire-protocol credential for an `airhouse_managed`
/// database via the SA-backed broker. Returns the
/// (host, port, username, password, dbname) tuple ready for the connector
/// constructor.
///
/// HTTP-handled paths arrive here with both `subject` (the requesting user
/// id, threaded by the workspace middleware) and `role` (the effective
/// workspace role) populated. Background paths (scheduler, schema crawl,
/// agentic background runs) leave `subject = None` and get
/// `AgenticBackground` at Admin.
///
/// **`sharing` decides whose name goes on the credential, and the caller must
/// say.** It is not a detail of this function: it follows from what the caller
/// does with the result, so getting it wrong writes a false audit row. See
/// [`CredentialUse`].
pub(super) async fn mint_airhouse_managed_creds<S>(
    workspace_manager: &WorkspaceManager<S>,
    subject: Option<uuid::Uuid>,
    role: Option<WorkspaceRole>,
    sharing: CredentialUse,
) -> Result<AirhouseWireParams, OxyError> {
    let workspace_id = workspace_manager.workspace_id;
    let endpoint = airhouse::wire_endpoint().ok_or_else(|| {
        OxyError::ConfigurationError(
            "airhouse_managed: AIRHOUSE_WIRE_HOST is not configured; the airhouse \
             integration must be enabled (set AIRHOUSE_BASE_URL, AIRHOUSE_ADMIN_TOKEN, \
             AIRHOUSE_WIRE_HOST, AIRHOUSE_WIRE_PORT)"
                .into(),
        )
    })?;
    let broker = airhouse::token_broker().ok_or_else(|| {
        OxyError::ConfigurationError(
            "airhouse_managed: token broker not initialised; check the airhouse env \
             vars (AIRHOUSE_BASE_URL / AIRHOUSE_ADMIN_TOKEN / AIRHOUSE_WIRE_HOST)"
                .into(),
        )
    })?;

    let cred = match (sharing, subject) {
        (CredentialUse::ReadOnly, _) => broker
            .mint_read_only(workspace_id, airhouse::DEFAULT_INTERNAL_TTL)
            .await
            .map_err(OxyError::from)?,
        (_, Some(uid)) => {
            let airhouse_role = role
                .map(airhouse::airhouse_role_for)
                // No effective role in scope (e.g. handler that didn't extract
                // EffectiveWorkspaceRole). Default to reader — least-privilege.
                .unwrap_or(airhouse::UserRole::Reader);
            // The role is the requester's either way, so neither arm grants
            // more than the other. Only the NAME on the credential differs,
            // and it follows who will use the connection.
            match sharing {
                CredentialUse::SharedPool | CredentialUse::ReadOnly => broker
                    .mint_for_system(
                        workspace_id,
                        airhouse::SystemPurpose::SharedQuery,
                        airhouse_role,
                        airhouse::DEFAULT_INTERNAL_TTL,
                    )
                    .await
                    .map_err(OxyError::from)?,
                CredentialUse::PerRequest => broker
                    .mint_for_user(
                        workspace_id,
                        uid,
                        airhouse_role,
                        airhouse::DEFAULT_INTERNAL_TTL,
                    )
                    .await
                    .map_err(OxyError::from)?,
            }
        }
        (_, None) => broker
            .mint_for_system(
                workspace_id,
                airhouse::SystemPurpose::AgenticBackground,
                airhouse::UserRole::Admin,
                airhouse::DEFAULT_INTERNAL_TTL,
            )
            .await
            .map_err(OxyError::from)?,
    };

    Ok(AirhouseWireParams {
        host: endpoint.host,
        port: endpoint.port,
        user: cred.username,
        password: cred.password,
        database: cred.tenant,
        // Carried, not dropped: airhouse closes a session whose credential has
        // lapsed, so this deadline is the only thing that lets the pool replace
        // the connection on its own schedule instead of being hung up on
        // mid-request. `expires_at` is UTC; negative is impossible but clamp
        // rather than wrap.
        expires_at: Some(cred.expires_at.timestamp().max(0) as u64),
    })
}
