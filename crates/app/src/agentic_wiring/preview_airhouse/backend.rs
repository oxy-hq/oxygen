//! Where the preview Airhouse connector gets a connection for a batch: the S5
//! port. Production mints preview credentials and pools them
//! ([`AirhouseBackend`]); tests hand out a fake, and a later slice a DuckDB
//! stand-in.

use std::sync::Arc;

use agentic_connector::{ConnectorError, DatabaseConnector};
use airhouse::preview_sql::PreviewNamespace;
use airhouse::{AirhouseConnector, UserRole};
use async_trait::async_trait;
use oxy_shared::errors::OxyError;
use uuid::Uuid;

use crate::agentic_wiring::airhouse_pool::{self, PREVIEW_KEY_PREFIX};

/// The credential a batch runs on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    /// A Reader: the batch writes nothing.
    Reader,
    /// A Writer confined to these preview schemas: sorted, never empty.
    Writer(Vec<String>),
}

/// How a batch uses its connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Use {
    /// No transaction: each statement commits on its own, so the connection
    /// is shared with every other caller of the same identity, as the rest of
    /// the Airhouse pool is. Reads and streams never wait on one another.
    Shared,
    /// A `BEGIN … COMMIT`: the connection is held for the batch alone, from
    /// identities nothing but transactions use, so no one else's statement
    /// lands inside the transaction.
    Transaction,
}

/// What keeps a [`Use::Transaction`] lease's connection exclusive.
pub trait Hold: Send + Sync {
    /// The connection may still be inside a transaction: never hand it out
    /// again.
    fn poison(&mut self);
}

/// The connection a batch runs on. For [`Use::Transaction`], nobody else is
/// handed it until this drops.
pub struct Lease {
    conn: Arc<dyn DatabaseConnector>,
    hold: Option<Box<dyn Hold>>,
}

impl Lease {
    /// A shared connection: nothing to hold, nothing to poison.
    pub fn new(conn: Arc<dyn DatabaseConnector>) -> Self {
        Self { conn, hold: None }
    }

    /// A connection kept exclusive by `hold` (a pool slot's guard, say) until
    /// the lease drops.
    pub fn held(conn: Arc<dyn DatabaseConnector>, hold: impl Hold + 'static) -> Self {
        Self {
            conn,
            hold: Some(Box::new(hold)),
        }
    }

    pub fn connector(&self) -> &dyn DatabaseConnector {
        self.conn.as_ref()
    }

    /// Never hand this connection out again (a no-op for a shared one, which
    /// never carries a transaction).
    pub fn poison(&mut self) {
        if let Some(hold) = self.hold.as_mut() {
            hold.poison();
        }
    }
}

impl Hold for airhouse_pool::ExclusiveLease {
    fn poison(&mut self) {
        airhouse_pool::ExclusiveLease::poison(self);
    }
}

#[async_trait]
pub trait PreviewAirhouseBackend: Send + Sync {
    /// A connection for one batch on `scope`, used as `usage` says.
    async fn checkout(&self, scope: &Scope, usage: Use) -> Result<Lease, ConnectorError>;
}

/// The workspace's Airhouse: a Reader on the analytics endpoint, or a Writer
/// on the primary one, each minted for the preview
/// (`AirhouseTokenBroker::mint_for_preview`, which refuses a Writer Airhouse
/// did not confine to the asked-for schemas) and pooled per identity.
pub struct AirhouseBackend {
    workspace_id: Uuid,
    ns: PreviewNamespace,
}

impl AirhouseBackend {
    pub fn new(workspace_id: Uuid, ns: PreviewNamespace) -> Self {
        Self { workspace_id, ns }
    }

    /// `preview:<workspace>:<key>` for the Reader,
    /// `preview:<workspace>:<key>:<schema>[,<schema>…]` for a Writer; a
    /// transaction's are `preview:tx:…`, so they are never shared.
    pub(super) fn pool_key(&self, scope: &Scope, usage: Use) -> String {
        let tx = match usage {
            Use::Shared => "",
            Use::Transaction => "tx:",
        };
        let base = format!(
            "{PREVIEW_KEY_PREFIX}{tx}{}:{}",
            self.workspace_id,
            self.ns.key()
        );
        match scope {
            Scope::Reader => base,
            Scope::Writer(schemas) => format!("{base}:{}", schemas.join(",")),
        }
    }
}

#[async_trait]
impl PreviewAirhouseBackend for AirhouseBackend {
    async fn checkout(&self, scope: &Scope, usage: Use) -> Result<Lease, ConnectorError> {
        let (workspace_id, ns, owned) = (self.workspace_id, self.ns.clone(), scope.clone());
        let key = self.pool_key(scope, usage);
        let build = || async move { connect(workspace_id, &ns, &owned).await };
        let lease = match usage {
            Use::Shared => airhouse_pool::get_or_build(key, build)
                .await
                .map(Lease::new),
            Use::Transaction => airhouse_pool::checkout_exclusive(key, build)
                .await
                .map(|held| Lease::held(Arc::clone(held.connector()), held)),
        };
        lease.map_err(|e| ConnectorError::ConnectionError(e.to_string()))
    }
}

/// Mint for the preview and open a wire connection. Writes go to the primary
/// endpoint, never the analytics pool.
async fn connect(
    workspace_id: Uuid,
    ns: &PreviewNamespace,
    scope: &Scope,
) -> Result<Arc<AirhouseConnector>, OxyError> {
    let (endpoint, schemas, role) = match scope {
        Scope::Reader => (
            airhouse::analytics_wire_endpoint().or_else(airhouse::wire_endpoint),
            &[][..],
            UserRole::Reader,
        ),
        Scope::Writer(schemas) => (
            airhouse::wire_endpoint(),
            schemas.as_slice(),
            UserRole::Writer,
        ),
    };
    let endpoint = endpoint.ok_or_else(|| {
        OxyError::ConfigurationError("Airhouse is not configured on this deployment".into())
    })?;
    let broker = airhouse::token_broker().ok_or_else(|| {
        OxyError::ConfigurationError("the Airhouse token broker is not initialised".into())
    })?;
    let cred = broker
        .mint_for_preview(
            workspace_id,
            ns,
            schemas,
            role,
            airhouse::DEFAULT_INTERNAL_TTL,
        )
        .await
        .map_err(OxyError::from)?;
    let conn = AirhouseConnector::new(
        &endpoint.host,
        endpoint.port,
        &cred.username,
        &cred.password,
        &cred.tenant,
    )
    .await
    .map_err(|e| {
        OxyError::DBError(format!(
            "preview Airhouse could not connect to {}:{}: {e}",
            endpoint.host, endpoint.port
        ))
    })?;
    Ok(Arc::new(conn))
}
