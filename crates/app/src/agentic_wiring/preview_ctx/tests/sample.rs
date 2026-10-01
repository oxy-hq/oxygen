//! An Airway sample's destination on the preview platform: the dataset mapped
//! into the preview's own schemas, each registered (and made) before the
//! destination is handed out, on a Writer minted for exactly those two; a
//! re-resolve of the mapped dataset keeps it; anything else is refused — and
//! an Airhouse that cannot confine a Writer refuses before anything exists.

use std::sync::{Arc, Mutex};

use ::airhouse::preview_sql::PreviewNamespace;
use agentic_pipeline::platform::ProjectContext;
use async_trait::async_trait;
use serde_json::json;

use super::fixture::{clickhouse_and_airhouse, row, seed_revision, seed_workspace};
use super::*;
use crate::agentic_wiring::preview_airhouse::{
    PipelineWriter, PreviewAirhouseBackend, PreviewAirhousePorts, Writers,
};
use crate::server::previews::airhouse_duckdb::DuckDbAirhouse;
use crate::server::previews::ddl::SchemaCreator;
use crate::server::test_support::{SKIP_MSG, test_db};

const KEY: &str = "feat_x_abc123";
pub(super) const DSN: &str = "postgresql://preview-writer:pw@airhouse.test:5445/tenant";

/// The in-process stand-in, answering the sample's Writer mint with `answer`
/// and recording every scope it was asked for.
pub(super) struct SampleAirhouse {
    inner: DuckDbAirhouse,
    answer: PipelineWriter,
    asked: Mutex<Vec<Vec<String>>>,
}

#[async_trait]
impl PreviewAirhousePorts for SampleAirhouse {
    fn backend(&self, ws: uuid::Uuid, ns: &PreviewNamespace) -> Arc<dyn PreviewAirhouseBackend> {
        self.inner.backend(ws, ns)
    }
    fn schema_creator(&self, ws: uuid::Uuid, ns: &PreviewNamespace) -> Arc<dyn SchemaCreator> {
        self.inner.schema_creator(ws, ns)
    }
    async fn deployment_writers(&self) -> Result<Writers, String> {
        self.inner.deployment_writers().await
    }
    async fn confine_writer(
        &self,
        ws: uuid::Uuid,
        ns: &PreviewNamespace,
        schemas: &[String],
    ) -> Result<Writers, String> {
        self.inner.confine_writer(ws, ns, schemas).await
    }
    fn catalog(&self) -> Option<String> {
        None
    }
    async fn pipeline_writer(
        &self,
        _ws: uuid::Uuid,
        _ns: &PreviewNamespace,
        schemas: &[String],
    ) -> Result<PipelineWriter, String> {
        self.asked.lock().unwrap().push(schemas.to_vec());
        Ok(self.answer.clone())
    }
}

pub(super) fn airhouse(answer: PipelineWriter) -> Arc<SampleAirhouse> {
    let conn = Arc::new(Mutex::new(duckdb::Connection::open_in_memory().unwrap()));
    Arc::new(SampleAirhouse {
        inner: DuckDbAirhouse::new(conn).expect("stand-in"),
        answer,
        asked: Mutex::new(Vec::new()),
    })
}

/// A sample run of a QuickBooks pipeline (`kind`) on a fresh workspace.
pub(super) async fn sample_row_of(
    db: &sea_orm::DatabaseConnection,
    kind: &str,
) -> entity::workspace_preview_runs::Model {
    let ws = seed_workspace(db).await;
    let staging = seed_revision(db, ws, "staging", clickhouse_and_airhouse()).await;
    let pipeline = json!({ "name": "qb", "source": { "kind": kind, "config": {} },
        "destination": { "database": "airhouse", "dataset_name": "quickbooks" } });
    super::fixture::exec(
        db,
        "INSERT INTO airway_pipelines (revision_id, name, file_path, definition) \
         VALUES ($1, 'qb', 'airway/qb.airway.yml', $2)",
        vec![staging.into(), pipeline.into()],
    )
    .await;
    let mut row = row(ws, staging);
    row.kind = SAMPLE_KIND.into();
    row.preview_key = KEY.into();
    row.target_ref = Some("airway/qb.airway.yml".into());
    row.options = json!({ "pipeline_name": "qb", "dataset_name": "quickbooks" });
    row
}

async fn sample_row(db: &sea_orm::DatabaseConnection) -> entity::workspace_preview_runs::Model {
    sample_row_of(db, "quickbooks").await
}

async fn registered(db: &sea_orm::DatabaseConnection, ws: uuid::Uuid) -> Vec<String> {
    use sea_orm::ConnectionTrait;
    db.query_all_raw(sea_orm::Statement::from_sql_and_values(
        sea_orm::DatabaseBackend::Postgres,
        "SELECT schema_name FROM workspace_preview_schemas WHERE workspace_id = $1 \
         ORDER BY schema_name",
        [ws.into()],
    ))
    .await
    .unwrap()
    .iter()
    .map(|r| r.try_get::<String>("", "schema_name").unwrap())
    .collect()
}

#[tokio::test]
async fn a_sample_lands_in_its_own_schemas_registered_first() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let row = sample_row(&db).await;
    let ports = airhouse(PipelineWriter::Dsn(DSN.into()));
    let ctx = PreviewPlatformContext::new_with(&db, &row, ports.clone())
        .await
        .expect("platform");

    let resolved = ctx
        .resolve_pipeline_destination("airhouse", "quickbooks")
        .await
        .expect("the sample's destination");
    let main = format!("preview_{KEY}__quickbooks");
    let raw = format!("preview_{KEY}__quickbooks_raw");
    assert_eq!(resolved.kind, "airhouse");
    assert_eq!(resolved.connection_string, DSN);
    assert_eq!(
        resolved.dataset_name_override.as_deref(),
        Some(main.as_str())
    );
    assert_eq!(
        ports.asked.lock().unwrap().clone(),
        vec![vec![main.clone(), raw.clone()]],
        "one Writer, confined to exactly the two"
    );
    assert_eq!(
        registered(&db, row.workspace_id).await,
        vec![main.clone(), raw]
    );

    // The worker's credential refresh hands the mapped dataset back.
    let again = ctx
        .resolve_pipeline_destination("airhouse", &main)
        .await
        .expect("re-resolved");
    assert_eq!(again.dataset_name_override.as_deref(), Some(main.as_str()));
    assert_eq!(ports.asked.lock().unwrap().len(), 2);

    // Anything but the managed Airhouse is refused, with why.
    assert!(
        ctx.resolve_pipeline_destination("clickhouse", "quickbooks")
            .await
            .is_none()
    );
    let why = ctx.sample().and_then(|s| s.refusal()).expect("a reason");
    assert!(why.contains("managed Airhouse"), "{why}");
}

/// I4/P4: an Airhouse that cannot confine a Writer (older than 0.1.49) refuses
/// the sample before any schema or registry row exists.
#[tokio::test]
async fn no_scoped_writer_refuses_the_sample_and_registers_nothing() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let row = sample_row(&db).await;
    let ports = airhouse(PipelineWriter::Unavailable(
        "stand-in for airhouse 0.1.48".into(),
    ));
    let ctx = PreviewPlatformContext::new_with(&db, &row, ports)
        .await
        .expect("platform");
    assert!(
        ctx.resolve_pipeline_destination("airhouse", "quickbooks")
            .await
            .is_none()
    );
    let why = ctx.sample().and_then(|s| s.refusal()).expect("a reason");
    assert!(
        why.starts_with(super::super::workspace::AIRHOUSE_TOO_OLD),
        "{why}"
    );
    assert!(why.contains("nothing was written"), "{why}");
    assert!(registered(&db, row.workspace_id).await.is_empty());
}

/// A procedure dry run's platform resolves no destination at all: its airway
/// steps are held.
#[tokio::test]
async fn only_a_samples_platform_resolves_a_destination() {
    let Some(db) = test_db().await else {
        eprintln!("{SKIP_MSG}");
        return;
    };
    let mut row = sample_row(&db).await;
    row.kind = "procedure".into();
    let ports = airhouse(PipelineWriter::Dsn(DSN.into()));
    let ctx = PreviewPlatformContext::new_with(&db, &row, ports.clone())
        .await
        .expect("platform");
    assert!(ctx.sample().is_none());
    assert!(
        ctx.resolve_pipeline_destination("airhouse", "quickbooks")
            .await
            .is_none()
    );
    assert!(ports.asked.lock().unwrap().is_empty(), "nothing minted");
}
