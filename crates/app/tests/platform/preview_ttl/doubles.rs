//! Test doubles around the DuckDB stand-in: one that watches the registry at
//! the moment a schema is created, and one that fails drops on demand.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use airhouse::preview_sql::PreviewNamespace;
use async_trait::async_trait;
use oxy_app::server::previews::ddl::{
    OpenSchemaDropper, PreviewDdlError, Relation, SchemaCreator, SchemaDropper,
};
use oxy_app::server::previews::ddl_duckdb::{DuckDbDroppers, DuckDbPreviewDdl};
use sea_orm::{DatabaseConnection, EntityTrait};
use uuid::Uuid;

/// Records, at the moment each schema is created, whether its registry row
/// already existed (and was live).
pub(super) struct Observing {
    pub db: DatabaseConnection,
    pub ws: Uuid,
    pub inner: DuckDbPreviewDdl,
    pub seen: std::sync::Mutex<Vec<(String, bool)>>,
}

#[async_trait]
impl SchemaCreator for Observing {
    async fn create_schema(&self, schema: &str) -> Result<(), PreviewDdlError> {
        let row =
            entity::workspace_preview_schemas::Entity::find_by_id((self.ws, schema.to_string()))
                .one(&self.db)
                .await
                .unwrap();
        let registered = row.is_some_and(|r| r.dropped_at.is_none());
        self.seen.lock().unwrap().push((schema.into(), registered));
        self.inner.create_schema(schema).await
    }
}

/// Fails every `DROP SCHEMA` while `fail` is set, as an unreachable Airhouse
/// would.
pub(super) struct Flaky {
    pub inner: DuckDbDroppers,
    pub fail: Arc<AtomicBool>,
}

struct FlakyDropper {
    inner: Box<dyn SchemaDropper>,
    fail: Arc<AtomicBool>,
}

impl OpenSchemaDropper for Flaky {
    fn open(&self, ws: Uuid, ns: &PreviewNamespace) -> Box<dyn SchemaDropper> {
        Box::new(FlakyDropper {
            inner: self.inner.open(ws, ns),
            fail: self.fail.clone(),
        })
    }
}

#[async_trait]
impl SchemaDropper for FlakyDropper {
    async fn list_relations(&self, schema: &str) -> Result<Vec<Relation>, PreviewDdlError> {
        self.inner.list_relations(schema).await
    }

    async fn drop_relation(
        &self,
        schema: &str,
        relation: &Relation,
    ) -> Result<(), PreviewDdlError> {
        self.inner.drop_relation(schema, relation).await
    }

    async fn drop_schema(&self, schema: &str) -> Result<(), PreviewDdlError> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(PreviewDdlError::Backend("Airhouse is unavailable".into()));
        }
        self.inner.drop_schema(schema).await
    }
}

/// Makes two callers' creates meet: the first `CREATE SCHEMA` to arrive waits
/// (up to [`Rendezvous::WAIT`]) for a second before either is sent, and
/// every create that reaches the stand-in is counted. When both callers get
/// this far they race head on, however loaded the test host is. When the
/// second never arrives (it is waiting on the first caller's row lock), the
/// first goes ahead after the wait.
pub(super) struct Rendezvous {
    pub inner: DuckDbPreviewDdl,
    pub creates: std::sync::atomic::AtomicUsize,
    arrivals: std::sync::atomic::AtomicUsize,
    second: tokio::sync::Notify,
}

impl Rendezvous {
    const WAIT: std::time::Duration = std::time::Duration::from_secs(2);

    pub fn new(inner: DuckDbPreviewDdl) -> Self {
        Self {
            inner,
            creates: Default::default(),
            arrivals: Default::default(),
            second: tokio::sync::Notify::new(),
        }
    }
}

#[async_trait]
impl SchemaCreator for Rendezvous {
    async fn create_schema(&self, schema: &str) -> Result<(), PreviewDdlError> {
        if self.arrivals.fetch_add(1, Ordering::SeqCst) == 0 {
            let _ = tokio::time::timeout(Self::WAIT, self.second.notified()).await;
        } else {
            // A stored permit: the first caller sees it even if it has not
            // started waiting yet.
            self.second.notify_one();
        }
        self.creates.fetch_add(1, Ordering::SeqCst);
        self.inner.create_schema(schema).await
    }
}
