//! Everything a preview run needs from an Airhouse, as one port: where its
//! batches run (the S5 [`PreviewAirhouseBackend`]), who creates its schemas
//! (the registry's [`SchemaCreator`]), whether this deployment can confine a
//! preview Writer at all, and the tenants' DuckLake catalog.
//!
//! Production is [`WorkspaceAirhouse`], the workspace's own Airhouse through
//! the token broker. Tests and in-process stand-ins pass another
//! (`previews::airhouse_duckdb`), so the whole preview write path — rewrite,
//! registry, copy-on-write, shadow map, verifier — runs against a real engine
//! without an Airhouse.

use std::sync::Arc;

use airhouse::BrokerError;
use airhouse::preview_sql::PreviewNamespace;
use async_trait::async_trait;
use uuid::Uuid;

use super::backend::{AirhouseBackend, PreviewAirhouseBackend};
use crate::server::previews::ddl::SchemaCreator;
use crate::server::previews::ddl_airhouse::AirhousePreviewDdl;

/// Whether preview writes can land on this Airhouse.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Writers {
    /// A Writer confined to the preview's schemas can be minted: writes land
    /// in the preview.
    Confined,
    /// It cannot (an Airhouse older than 0.1.49, one that did not answer, or
    /// none at all): the preview's writes stay held, as in phase 2a. Never a
    /// live write.
    Unavailable(String),
}

#[async_trait]
pub trait PreviewAirhousePorts: Send + Sync {
    /// Where preview `ns`'s batches run.
    fn backend(&self, workspace_id: Uuid, ns: &PreviewNamespace)
    -> Arc<dyn PreviewAirhouseBackend>;

    /// Who creates preview `ns`'s schemas, after their registry rows.
    fn schema_creator(&self, workspace_id: Uuid, ns: &PreviewNamespace) -> Arc<dyn SchemaCreator>;

    /// Asked before a step's writes are prepared: can this deployment confine
    /// a preview Writer at all? `Err` when it could not be asked.
    async fn deployment_writers(&self) -> Result<Writers, String>;

    /// Asked once the step's schemas are known, before any of them is
    /// created: a Writer confined to exactly `schemas`. `Err` is a refusal
    /// or a failure that fails the step.
    async fn confine_writer(
        &self,
        workspace_id: Uuid,
        ns: &PreviewNamespace,
        schemas: &[String],
    ) -> Result<Writers, String>;

    /// The tenants' DuckLake catalog, when the deployment names it. `None`
    /// refuses every catalog-qualified name.
    fn catalog(&self) -> Option<String>;

    /// A Postgres-wire DSN on a Writer confined to exactly `schemas`: an
    /// Airway sample's destination. The default — every stand-in's answer — is
    /// [`PipelineWriter::Unavailable`]: a sample lands only where a confined
    /// Writer can be minted, and is refused everywhere else.
    async fn pipeline_writer(
        &self,
        _workspace_id: Uuid,
        _ns: &PreviewNamespace,
        _schemas: &[String],
    ) -> Result<PipelineWriter, String> {
        Ok(PipelineWriter::Unavailable(
            "this Airhouse lands no Airway samples".into(),
        ))
    }
}

/// Where an Airway sample may land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PipelineWriter {
    /// A DSN whose credential writes only the preview schemas it was minted for.
    Dsn(String),
    /// No confined Writer here (an Airhouse older than 0.1.49, or none): the
    /// sample is refused before anything is written.
    Unavailable(String),
}

/// The workspace's Airhouse: preview credentials from the token broker,
/// schemas created on a system Writer (`ddl_airhouse`).
#[derive(Clone, Copy, Debug, Default)]
pub struct WorkspaceAirhouse;

impl WorkspaceAirhouse {
    pub fn shared() -> Arc<dyn PreviewAirhousePorts> {
        Arc::new(Self)
    }
}

const NOT_CONFIGURED: &str = "Airhouse is not configured on this deployment";

#[async_trait]
impl PreviewAirhousePorts for WorkspaceAirhouse {
    fn backend(
        &self,
        workspace_id: Uuid,
        ns: &PreviewNamespace,
    ) -> Arc<dyn PreviewAirhouseBackend> {
        Arc::new(AirhouseBackend::new(workspace_id, ns.clone()))
    }

    fn schema_creator(&self, workspace_id: Uuid, ns: &PreviewNamespace) -> Arc<dyn SchemaCreator> {
        Arc::new(AirhousePreviewDdl::new(workspace_id, ns.clone()))
    }

    async fn deployment_writers(&self) -> Result<Writers, String> {
        let Some(broker) = airhouse::token_broker() else {
            return Ok(Writers::Unavailable(NOT_CONFIGURED.into()));
        };
        match broker.scopes_preview_writers().await {
            Ok(true) => Ok(Writers::Confined),
            Ok(false) => Ok(Writers::Unavailable(
                BrokerError::ScopedWritersUnsupported.to_string(),
            )),
            Err(e) => writers_after(e),
        }
    }

    async fn confine_writer(
        &self,
        workspace_id: Uuid,
        ns: &PreviewNamespace,
        schemas: &[String],
    ) -> Result<Writers, String> {
        let Some(broker) = airhouse::token_broker() else {
            return Ok(Writers::Unavailable(NOT_CONFIGURED.into()));
        };
        let minted = broker
            .mint_for_preview(
                workspace_id,
                ns,
                schemas,
                airhouse::UserRole::Writer,
                airhouse::DEFAULT_INTERNAL_TTL,
            )
            .await;
        match minted {
            // Cached by the broker: the batch's own mint finds it.
            Ok(_) => Ok(Writers::Confined),
            Err(e) => writers_after(e),
        }
    }

    fn catalog(&self) -> Option<String> {
        airhouse::ducklake_catalog()
    }

    /// Minted like a step's batch Writer (`mint_for_preview`, which refuses a
    /// Writer Airhouse did not confine), on the primary wire endpoint.
    async fn pipeline_writer(
        &self,
        workspace_id: Uuid,
        ns: &PreviewNamespace,
        schemas: &[String],
    ) -> Result<PipelineWriter, String> {
        let (Some(broker), Some(endpoint)) = (airhouse::token_broker(), airhouse::wire_endpoint())
        else {
            return Ok(PipelineWriter::Unavailable(NOT_CONFIGURED.into()));
        };
        let minted = broker
            .mint_for_preview(
                workspace_id,
                ns,
                schemas,
                airhouse::UserRole::Writer,
                airhouse::DEFAULT_INTERNAL_TTL,
            )
            .await;
        let cred = match minted {
            Ok(cred) => cred,
            Err(e) => {
                return match writers_after(e)? {
                    Writers::Unavailable(why) => Ok(PipelineWriter::Unavailable(why)),
                    Writers::Confined => Err("a refused mint read as confined".into()),
                };
            }
        };
        Ok(PipelineWriter::Dsn(format!(
            "postgresql://{}:{}@{}:{}/{}",
            urlencoding::encode(&cred.username),
            urlencoding::encode(&cred.password),
            endpoint.host,
            endpoint.port,
            urlencoding::encode(&cred.tenant),
        )))
    }
}

/// A broker error, read as "this Airhouse cannot confine a preview Writer"
/// (writes stay held) or as a failure of the step.
fn writers_after(e: BrokerError) -> Result<Writers, String> {
    match e {
        BrokerError::ScopedWritersUnsupported
        | BrokerError::UnscopedWriter { .. }
        | BrokerError::CapabilitiesUnanswered(_) => Ok(Writers::Unavailable(e.to_string())),
        other => Err(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_writer_airhouse_cannot_confine_holds_everything_else_fails() {
        for held in [
            BrokerError::ScopedWritersUnsupported,
            BrokerError::UnscopedWriter {
                asked: vec!["preview_k_abc123__s".into()],
                echoed: None,
            },
            BrokerError::CapabilitiesUnanswered(std::time::Duration::from_secs(10)),
        ] {
            assert!(
                matches!(writers_after(held), Ok(Writers::Unavailable(_))),
                "held"
            );
        }
        let refused = BrokerError::PreviewScope("too long".into());
        assert!(writers_after(refused).is_err(), "a refusal fails the step");
    }
}
