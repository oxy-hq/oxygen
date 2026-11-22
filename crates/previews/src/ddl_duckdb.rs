//! In-process DuckDB behind the preview DDL port: the stand-in for Airhouse
//! that the TTL tests drive. It sends exactly the statements
//! [`super::ddl::Ddl`] builds, so a test exercises the SQL production sends,
//! and it refuses the same names. Not used by the server.

use std::sync::{Arc, Mutex};

use airhouse::preview_sql::PreviewNamespace;
use async_trait::async_trait;
use uuid::Uuid;

use super::ddl::{
    Ddl, OpenSchemaDropper, PreviewDdlError, Relation, RelationKind, SchemaCreator, SchemaDropper,
    create_failed, list_relations_sql, schema_exists_sql,
};

/// One preview's DDL against a shared DuckDB connection.
pub struct DuckDbPreviewDdl {
    conn: Arc<Mutex<duckdb::Connection>>,
    ns: PreviewNamespace,
}

impl DuckDbPreviewDdl {
    pub fn new(conn: Arc<Mutex<duckdb::Connection>>, ns: PreviewNamespace) -> Self {
        Self { conn, ns }
    }

    fn execute(&self, ddl: Ddl) -> Result<(), PreviewDdlError> {
        let sql = ddl.statement(&self.ns)?;
        self.conn
            .lock()
            .map_err(|_| PreviewDdlError::Backend("duckdb connection poisoned".into()))?
            .execute_batch(&sql)
            .map_err(|e| PreviewDdlError::Backend(format!("{sql}: {e}")))
    }
}

#[async_trait]
impl SchemaCreator for DuckDbPreviewDdl {
    async fn create_schema(&self, schema: &str) -> Result<(), PreviewDdlError> {
        let Err(error) = self.execute(Ddl::CreateSchema(schema.to_string())) else {
            return Ok(());
        };
        let PreviewDdlError::Backend(message) = error else {
            return Err(error);
        };
        let probe = schema_exists_sql(&self.ns, schema)?;
        let exists = self
            .conn
            .lock()
            .map_err(|_| PreviewDdlError::Backend("duckdb connection poisoned".into()))?
            .query_row(&probe, [], |row| row.get::<_, i64>(0))
            .map_err(|e| PreviewDdlError::Backend(format!("{probe}: {e}")))?;
        Err(create_failed(schema, exists > 0, message))
    }
}

#[async_trait]
impl SchemaDropper for DuckDbPreviewDdl {
    async fn list_relations(&self, schema: &str) -> Result<Vec<Relation>, PreviewDdlError> {
        let sql = list_relations_sql(&self.ns, schema)?;
        let backend = |e: duckdb::Error| PreviewDdlError::Backend(format!("{sql}: {e}"));
        let conn = self
            .conn
            .lock()
            .map_err(|_| PreviewDdlError::Backend("duckdb connection poisoned".into()))?;
        let mut stmt = conn.prepare(&sql).map_err(backend)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(Relation {
                    name: row.get::<_, String>(0)?,
                    kind: RelationKind::from_table_type(&row.get::<_, String>(1)?),
                })
            })
            .map_err(backend)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(backend)
    }

    async fn drop_relation(
        &self,
        schema: &str,
        relation: &Relation,
    ) -> Result<(), PreviewDdlError> {
        self.execute(Ddl::DropRelation(schema.to_string(), relation.clone()))
    }

    async fn drop_schema(&self, schema: &str) -> Result<(), PreviewDdlError> {
        self.execute(Ddl::DropSchema(schema.to_string()))
    }
}

/// Hands every preview the same DuckDB connection.
#[derive(Clone)]
pub struct DuckDbDroppers {
    pub conn: Arc<Mutex<duckdb::Connection>>,
}

impl OpenSchemaDropper for DuckDbDroppers {
    fn open(&self, _workspace_id: Uuid, ns: &PreviewNamespace) -> Box<dyn SchemaDropper> {
        Box::new(DuckDbPreviewDdl::new(self.conn.clone(), ns.clone()))
    }
}
