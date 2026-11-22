//! Airhouse reads in a preview run: a Reader credential, never the admin one.
//!
//! A background context with no subject mints `airhouse_managed` credentials
//! as admin (`mint_for_system(AgenticBackground, Admin)`). A preview reads live
//! tables and writes nothing, so it mints its own Reader for
//! `SystemPurpose::Preview` — the credential the Airway change check already
//! uses. The connector is wrapped in `HoldingConnector` by the caller like
//! every other: the credential and the wrapper are two separate fences.

use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use uuid::Uuid;

pub(super) async fn reader_connector(
    workspace_id: Uuid,
) -> Result<Arc<dyn DatabaseConnector>, String> {
    let endpoint = airhouse::analytics_wire_endpoint()
        .or_else(airhouse::wire_endpoint)
        .ok_or("Airhouse is not configured on this deployment")?;
    let broker = airhouse::token_broker().ok_or("the Airhouse token broker is not initialised")?;
    let cred = broker
        .mint_for_system(
            workspace_id,
            airhouse::SystemPurpose::Preview,
            airhouse::UserRole::Reader,
            airhouse::DEFAULT_INTERNAL_TTL,
        )
        .await
        .map_err(|e| format!("minting a Reader credential: {e}"))?;
    let conn = airhouse::AirhouseConnector::new(
        &endpoint.host,
        endpoint.port,
        &cred.username,
        &cred.password,
        &cred.tenant,
    )
    .await
    .map_err(|e| format!("connecting to Airhouse as a Reader: {e}"))?;
    Ok(Arc::new(conn))
}
