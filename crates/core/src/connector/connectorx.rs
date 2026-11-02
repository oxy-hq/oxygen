use std::sync::Arc;

use arrow::{array::RecordBatch, datatypes::SchemaRef};
use bigquery::BigQuerySource;
use bigquery_transport::BigQueryArrowTransport;
use connectorx::prelude::{ArrowDestination, CXQuery, Dispatcher, SourceConn, get_arrow};
use df_interchange::Interchange;
use uuid::Uuid;

use oxy_shared::errors::OxyError;

use super::{
    constants::{
        BIGQUERY_DIALECT, CREATE_CONN, EXECUTE_QUERY, FAILED_TO_RUN_BLOCKING_TASK,
        LOAD_ARROW_RESULT, WRITE_RESULT,
    },
    engine::Engine,
    utils::{connector_internal_error, write_to_ipc},
};

mod bigquery;
mod bigquery_transport;

pub(super) struct ConnectorX {
    dialect: String,
    db_path: String,
    dry_run_limit: Option<u64>,
    /// BigQuery only: the service-account key JSON, when it came from the
    /// secret store (`key_json_var`). Set, it is used instead of reading a key
    /// file at `db_path`.
    bigquery_key_json: Option<Arc<str>>,
}

impl std::fmt::Debug for ConnectorX {
    // By hand: `db_path` is a path for BigQuery but `user:password@host` for
    // the Postgres-wire dialects, and the key JSON is a credential.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorX")
            .field("dialect", &self.dialect)
            .field("dry_run_limit", &self.dry_run_limit)
            .field("bigquery_key_json", &self.bigquery_key_json.is_some())
            .finish_non_exhaustive()
    }
}

impl ConnectorX {
    pub fn new(dialect: String, db_path: String, dry_run_limit: Option<u64>) -> Self {
        Self {
            dialect,
            db_path,
            dry_run_limit,
            bigquery_key_json: None,
        }
    }

    /// Authenticate BigQuery with the key's contents rather than a key file.
    pub fn with_bigquery_key_json(mut self, key_json: String) -> Self {
        self.bigquery_key_json = Some(Arc::from(key_json));
        self
    }

    /// The BigQuery source for one query: from the key JSON when there is
    /// one, else from the key file named in the connection string.
    fn bigquery_source(
        &self,
        rt: Arc<tokio::runtime::Runtime>,
        dry_run_limit: Option<u64>,
    ) -> impl FnOnce() -> Result<BigQuerySource, OxyError> + Send + 'static {
        let conn_string = format!("{}://{}", self.dialect, self.db_path);
        let key_json = self.bigquery_key_json.clone();
        move || {
            match key_json {
                Some(json) => BigQuerySource::from_key_json(rt, &json, dry_run_limit),
                None => BigQuerySource::new(rt, &conn_string, dry_run_limit),
            }
            .map_err(|err| connector_internal_error(CREATE_CONN, &err))
        }
    }
}

impl Engine for ConnectorX {
    async fn dry_run(&self, query: &str) -> Result<(Vec<RecordBatch>, SchemaRef), OxyError> {
        match self.dialect.as_str() {
            BIGQUERY_DIALECT => {
                let rt =
                    Arc::new(tokio::runtime::Runtime::new().map_err(|err| {
                        connector_internal_error(FAILED_TO_RUN_BLOCKING_TASK, &err)
                    })?);
                let query = query.to_string();
                let source = self.bigquery_source(rt, None);

                tokio::task::spawn_blocking(move || {
                    let mut source = source()?;
                    source
                        .dry_run(&query)
                        .map_err(|err| connector_internal_error(EXECUTE_QUERY, &err))
                })
                .await
                .map_err(|err| connector_internal_error(FAILED_TO_RUN_BLOCKING_TASK, &err))?
            }
            _ => self.explain_query(query).await,
        }
    }

    async fn explain_query(&self, query: &str) -> Result<(Vec<RecordBatch>, SchemaRef), OxyError> {
        match self.dialect.as_str() {
            BIGQUERY_DIALECT => Box::pin(self.dry_run(query)).await, // We're forming the loop here because the dialect is bigquery
            _ => {
                let explain_query = format!("EXPLAIN ({})", query.trim().trim_end_matches(';'));
                self.run_query_with_limit(&explain_query, None).await
            }
        }
    }

    async fn run_query(&self, query: &str) -> Result<String, OxyError> {
        let (record_batches, schema_ref) =
            self.run_query_with_limit(query, self.dry_run_limit).await?;
        let file_path = format!("/tmp/{}.arrow", Uuid::new_v4());
        write_to_ipc(&record_batches, &file_path, &schema_ref)
            .map_err(|err| connector_internal_error(WRITE_RESULT, err.as_ref()))?;
        Ok(file_path)
    }

    async fn run_query_with_limit(
        &self,
        query: &str,
        dry_run_limit: Option<u64>,
    ) -> Result<(Vec<RecordBatch>, SchemaRef), OxyError> {
        let conn_string = format!("{}://{}", self.dialect, self.db_path);
        let query = query.to_string();
        let dialect = self.dialect.clone();
        // Prepared here because it borrows `self`; it is built on the blocking
        // thread like the rest.
        let bigquery_source = if dialect == BIGQUERY_DIALECT {
            let rt = Arc::new(
                tokio::runtime::Runtime::new()
                    .map_err(|err| connector_internal_error(FAILED_TO_RUN_BLOCKING_TASK, &err))?,
            );
            Some(self.bigquery_source(rt, dry_run_limit))
        } else {
            None
        };
        let result = tokio::task::spawn_blocking(move || {
            let destination = match dialect.as_str() {
                BIGQUERY_DIALECT => {
                    let mut destination = ArrowDestination::new();
                    let source = bigquery_source.expect("built for the bigquery dialect")()?;
                    let queries = &[query.as_str()];
                    let dispatcher = Dispatcher::<_, _, BigQueryArrowTransport>::new(
                        source,
                        &mut destination,
                        queries,
                        None,
                    );
                    dispatcher
                        .run()
                        .map_err(|err| connector_internal_error(EXECUTE_QUERY, &err))?;
                    Ok(destination)
                }
                _ => {
                    let source_conn = SourceConn::try_from(conn_string.as_str())
                        .map_err(|err| connector_internal_error(CREATE_CONN, &err))?;
                    let queries = &[CXQuery::from(query.as_str())];
                    get_arrow(&source_conn, None, queries, None)
                        .map_err(|err| connector_internal_error(EXECUTE_QUERY, &err))
                }
            }?;
            let _schema = destination.arrow_schema();
            let result = destination
                .arrow()
                .map_err(|err| connector_internal_error(LOAD_ARROW_RESULT, &err))?;
            // `Interchange::from_arrow_54` shares the same empty-vec panic
            // as `_58` (indexes `df[0]` without a guard). Return empty so the
            // `result.first()` check below yields the clean "No record batches
            // returned" error rather than panicking — zero rows is an error on
            // this path, unlike duckdb.rs where it's an empty-result success.
            if result.is_empty() {
                return Result::<_, OxyError>::Ok(Vec::new());
            }
            let converted_result = Interchange::from_arrow_54(result)
                .map_err(|err| connector_internal_error(LOAD_ARROW_RESULT, &err))?
                .to_arrow_58()
                .map_err(|err| connector_internal_error(LOAD_ARROW_RESULT, &err))?;
            Result::<_, OxyError>::Ok(converted_result)
        })
        .await
        .map_err(|e| connector_internal_error(FAILED_TO_RUN_BLOCKING_TASK, &e))??;
        let schema = result.first().map(|batch| batch.schema()).ok_or_else(|| {
            OxyError::RuntimeError("No record batches returned from query".to_string())
        })?;
        Ok((result, schema))
    }
}
