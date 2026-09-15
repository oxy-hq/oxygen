//! The Airhouse connection a custom app writes its own facts through.
//!
//! Minted for the app (`BrokerSubject::App`), never for whoever invoked the
//! function, so a schedule, a webhook and a click write the same way. Pooled
//! per app like every other Airhouse identity, and on the serving endpoint
//! rather than the analytics pool, because these are writes.

use std::sync::Arc;

use agentic_connector::DatabaseConnector;
use oxy_shared::errors::OxyError;
use uuid::Uuid;

/// Connect as `app_slug`, asking Airhouse to confine writes to `schema`.
///
/// An Airhouse that predates scoped credentials returns a tenant-wide Writer.
/// That is logged, not refused: the host checks every statement against the
/// schema before it is sent (`airhouse::sql_rules`), so the scope is a second
/// fence, not the only one.
pub async fn connector(
    workspace_id: Uuid,
    app_slug: &str,
    schema: &str,
) -> Result<Arc<dyn DatabaseConnector>, OxyError> {
    let key = format!("app:{workspace_id}:{app_slug}");
    let (slug, schema) = (app_slug.to_string(), schema.to_string());
    super::airhouse_pool::get_or_build(key, || async move {
        let endpoint = airhouse::wire_endpoint().ok_or_else(|| {
            OxyError::ConfigurationError(
                "ctx.airhouse: AIRHOUSE_WIRE_HOST is not configured; the airhouse integration \
                 must be enabled"
                    .into(),
            )
        })?;
        let broker = airhouse::token_broker().ok_or_else(|| {
            OxyError::ConfigurationError(
                "ctx.airhouse: the Airhouse token broker is not initialised".into(),
            )
        })?;
        let cred = broker
            .mint_for_app(
                workspace_id,
                &slug,
                &schema,
                airhouse::UserRole::Writer,
                airhouse::DEFAULT_INTERNAL_TTL,
            )
            .await
            .map_err(OxyError::from)?;
        if cred.write_schemas.is_none() {
            tracing::warn!(
                %workspace_id,
                app = %slug,
                %schema,
                "Airhouse returned an unscoped Writer for this app (it predates write_schemas); \
                 its writes are confined by the host's statement check alone"
            );
        }
        let conn = airhouse::AirhouseConnector::new(
            &endpoint.host,
            endpoint.port,
            &cred.username,
            &cred.password,
            &cred.tenant,
        )
        .await
        .map_err(|e| {
            OxyError::DBError(format!(
                "ctx.airhouse could not connect to {}:{}: {e}",
                endpoint.host, endpoint.port
            ))
        })?;
        Ok(Arc::new(conn))
    })
    .await
}
