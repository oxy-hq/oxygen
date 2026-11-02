// Forked from connector-x
// Source implementation for Google BigQuery

use anyhow::anyhow;
use arrow::{
    array::{Array, ArrayData, FixedSizeListArray, GenericByteArray, RecordBatch, StringArray},
    datatypes::{DataType, Field, Schema, SchemaRef, Utf8Type},
};
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use connectorx::{
    data_order::DataOrder,
    errors::ConnectorXError,
    sources::{
        PartitionParser, Produce, Source, SourcePartition,
        bigquery::{BigQuerySourceError, BigQueryTypeSystem},
    },
    sql::{CXQuery, count_query, limit1_query},
};
use fehler::{throw, throws};
use gcp_bigquery_client::{
    Client,
    model::{
        get_query_results_parameters::GetQueryResultsParameters,
        get_query_results_response::GetQueryResultsResponse, query_request::QueryRequest,
        query_response::ResultSet,
    },
};
use sqlparser_connectorx::dialect::Dialect;
use std::sync::Arc;
use tokio::runtime::Runtime;
use url::Url;

const DEFAULT_TIMEOUT_MS: i32 = 180000;

fn make_query_request_with_default_timeout(sql_query: impl Into<String>) -> QueryRequest {
    QueryRequest {
        connection_properties: None,
        default_dataset: None,
        dry_run: None,
        kind: None,
        labels: None,
        location: None,
        max_results: None,
        maximum_bytes_billed: None,
        parameter_mode: None,
        preserve_nulls: None,
        query: sql_query.into(),
        query_parameters: None,
        request_id: None,
        timeout_ms: Some(DEFAULT_TIMEOUT_MS),
        use_legacy_sql: false, // force standard SQL by default
        use_query_cache: None,
        format_options: None,
    }
}

fn make_dry_run_query_request(sql_query: impl Into<String>) -> QueryRequest {
    QueryRequest {
        connection_properties: None,
        default_dataset: None,
        dry_run: Some(true),
        kind: None,
        labels: None,
        location: None,
        max_results: None,
        maximum_bytes_billed: None,
        parameter_mode: None,
        preserve_nulls: None,
        query: sql_query.into(),
        query_parameters: None,
        request_id: None,
        timeout_ms: Some(DEFAULT_TIMEOUT_MS),
        use_legacy_sql: false, // force standard SQL by default
        use_query_cache: None,
        format_options: None,
    }
}

#[derive(Debug)]
pub struct BigQueryDialect {}

impl Dialect for BigQueryDialect {
    // See https://cloud.google.com/bigquery/docs/reference/standard-sql/lexical
    fn is_delimited_identifier_start(&self, ch: char) -> bool {
        ch == '`'
    }

    fn is_identifier_start(&self, ch: char) -> bool {
        ch.is_ascii_lowercase() || ch.is_ascii_uppercase() || ch == '_' || ch == '-'
    }

    fn is_identifier_part(&self, ch: char) -> bool {
        self.is_identifier_start(ch) || ch.is_ascii_digit()
    }
}

pub struct BigQuerySource {
    rt: Arc<Runtime>,
    client: Arc<Client>,
    project_id: String,
    origin_query: Option<String>,
    queries: Vec<CXQuery<String>>,
    names: Vec<String>,
    schema: Vec<BigQueryTypeSystem>,
    dry_run_limit: Option<u64>,
}

impl BigQuerySource {
    #[throws(BigQuerySourceError)]
    pub fn new(rt: Arc<Runtime>, conn: &str, dry_run_limit: Option<u64>) -> Self {
        let url = Url::parse(conn)?;
        let sa_key_path = url.path();
        let client = Arc::new(rt.block_on(
            gcp_bigquery_client::Client::from_service_account_key_file(sa_key_path),
        )?);
        let auth_data = std::fs::read_to_string(sa_key_path)?;
        let auth_json: serde_json::Value = serde_json::from_str(&auth_data)?;
        let project_id = auth_json
            .get("project_id")
            .ok_or_else(|| anyhow!("Cannot get project_id from auth file"))?
            .as_str()
            .ok_or_else(|| anyhow!("Cannot get project_id as string from auth file"))?
            .to_string();
        Self {
            rt,
            client,
            project_id,
            origin_query: None,
            queries: vec![],
            names: vec![],
            schema: vec![],
            dry_run_limit,
        }
    }

    /// [`BigQuerySource::new`] from the service-account key JSON itself (a
    /// `key_json_var` secret) instead of a key file: nothing is written to disk.
    #[throws(BigQuerySourceError)]
    pub fn from_key_json(rt: Arc<Runtime>, key_json: &str, dry_run_limit: Option<u64>) -> Self {
        let sa_key = gcp_bigquery_client::yup_oauth2::parse_service_account_key(key_json)?;
        let client = Arc::new(rt.block_on(
            gcp_bigquery_client::Client::from_service_account_key(sa_key, false),
        )?);
        let auth_json: serde_json::Value = serde_json::from_str(key_json)?;
        let project_id = auth_json
            .get("project_id")
            .ok_or_else(|| anyhow!("Cannot get project_id from the service-account key"))?
            .as_str()
            .ok_or_else(|| anyhow!("Cannot get project_id as string from the service-account key"))?
            .to_string();
        Self {
            rt,
            client,
            project_id,
            origin_query: None,
            queries: vec![],
            names: vec![],
            schema: vec![],
            dry_run_limit,
        }
    }

    #[throws(BigQuerySourceError)]
    pub fn dry_run(&mut self, query: &str) -> (Vec<RecordBatch>, SchemaRef) {
        let job = self.client.job();
        let rs = self
            .rt
            .block_on(job.query(self.project_id.as_str(), make_dry_run_query_request(query)))?;
        let errors_list = DataType::FixedSizeList(
            Arc::new(Field::new("error", DataType::Utf8, false)),
            rs.errors.as_ref().map(|v| v.len()).unwrap_or(0) as i32,
        );
        let schema = Arc::new(Schema::new(vec![
            Field::new("total_bytes_processed", DataType::Utf8, true),
            Field::new("errors", errors_list.clone(), true),
        ]));
        let batches = vec![
            RecordBatch::try_new(
                schema.clone(),
                vec![
                    Arc::new(StringArray::from(vec![rs.total_bytes_processed.clone()])),
                    Arc::new(FixedSizeListArray::from(
                        ArrayData::builder(errors_list)
                            .len(1)
                            .add_child_data(
                                GenericByteArray::<Utf8Type>::from(
                                    rs.errors
                                        .map(|v| {
                                            v.into_iter().map(|e| e.to_string()).collect::<Vec<_>>()
                                        })
                                        .unwrap_or_default(),
                                )
                                .into_data(),
                            )
                            .build()
                            .map_err(|err| anyhow!(err.to_string()))?,
                    )),
                ],
            )
            .map_err(|err| anyhow!(err.to_string()))?,
        ];
        (batches, schema)
    }
}

impl Source for BigQuerySource
where
    BigQuerySourcePartition:
        SourcePartition<TypeSystem = BigQueryTypeSystem, Error = BigQuerySourceError>,
{
    const DATA_ORDERS: &'static [DataOrder] = &[DataOrder::RowMajor];
    type Partition = BigQuerySourcePartition;
    type TypeSystem = BigQueryTypeSystem;
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn set_data_order(&mut self, data_order: DataOrder) {
        if !matches!(data_order, DataOrder::RowMajor) {
            throw!(ConnectorXError::UnsupportedDataOrder(data_order));
        }
    }

    fn set_queries<Q: ToString>(&mut self, queries: &[CXQuery<Q>]) {
        self.queries = queries.iter().map(|q| q.map(Q::to_string)).collect();
    }

    fn set_origin_query(&mut self, query: Option<String>) {
        self.origin_query = query;
    }

    #[throws(BigQuerySourceError)]
    fn fetch_metadata(&mut self) {
        assert!(!self.queries.is_empty());
        let job = self.client.job();
        for query in self.queries.iter() {
            // Check bytes limit
            tracing::debug!("Dry run limit: {:?}", self.dry_run_limit);
            if let Some(dry_run_limit) = self.dry_run_limit {
                let dry_run_rs = self.rt.block_on(job.query(
                    self.project_id.as_str(),
                    make_dry_run_query_request(query.as_str()),
                ))?;
                tracing::debug!("Dry run response: {:?}", dry_run_rs);
                if let Some(total_bytes_processed) = dry_run_rs
                    .total_bytes_processed
                    .and_then(|v| v.parse::<u64>().ok())
                    && total_bytes_processed > dry_run_limit
                {
                    throw!(anyhow!(
                        "Query would process {} bytes of data, which would exceed the dry run limit of {} bytes.",
                        total_bytes_processed,
                        dry_run_limit
                    ));
                }
            }

            // Run limit 1 query to fetch schema
            let l1query = limit1_query(query, &BigQueryDialect {})?;
            let rs = self.rt.block_on(job.query(
                self.project_id.as_str(),
                make_query_request_with_default_timeout(l1query.as_str()),
            ))?;
            let (names, types) = rs
                .schema
                .as_ref()
                .ok_or_else(|| anyhow!("TableSchema is none"))?
                .fields
                .as_ref()
                .ok_or_else(|| anyhow!("TableFieldSchema is none"))?
                .iter()
                .map(|col| {
                    (
                        col.clone().name,
                        BigQueryTypeSystem::from(&col.clone().r#type),
                    )
                })
                .unzip();
            self.names = names;
            self.schema = types;
        }
    }

    #[throws(BigQuerySourceError)]
    fn result_rows(&mut self) -> Option<usize> {
        match &self.origin_query {
            Some(q) => {
                let cxq = CXQuery::Naked(q.clone());
                let cquery = count_query(&cxq, &BigQueryDialect {})?;
                let job = self.client.job();
                let query_response = self.rt.block_on(job.query(
                    self.project_id.as_str(),
                    make_query_request_with_default_timeout(cquery.as_str()),
                ))?;
                let mut rs = ResultSet::new_from_query_response(query_response);
                rs.next_row();
                let nrows = rs
                    .get_i64(0)?
                    .ok_or_else(|| anyhow!("cannot get row number"))?;
                Some(nrows as usize)
            }
            None => None,
        }
    }

    fn names(&self) -> Vec<String> {
        self.names.clone()
    }

    fn schema(&self) -> Vec<Self::TypeSystem> {
        self.schema.clone()
    }

    #[throws(BigQuerySourceError)]
    fn partition(self) -> Vec<Self::Partition> {
        let mut ret = vec![];
        for query in self.queries {
            ret.push(BigQuerySourcePartition::new(
                self.rt.clone(),
                self.client.clone(),
                self.project_id.clone(),
                &query,
                &self.schema,
            ));
        }
        ret
    }
}

pub struct BigQuerySourcePartition {
    rt: Arc<Runtime>,
    client: Arc<Client>,
    project_id: String,
    query: CXQuery<String>,
    schema: Vec<BigQueryTypeSystem>,
    nrows: usize,
    ncols: usize,
}

impl BigQuerySourcePartition {
    pub fn new(
        handle: Arc<Runtime>,
        client: Arc<Client>,
        project_id: String,
        query: &CXQuery<String>,
        schema: &[BigQueryTypeSystem],
    ) -> Self {
        Self {
            rt: handle,
            client,
            project_id: project_id.clone(),
            query: query.clone(),
            schema: schema.to_vec(),
            nrows: 0,
            ncols: schema.len(),
        }
    }
}

impl SourcePartition for BigQuerySourcePartition {
    type TypeSystem = BigQueryTypeSystem;
    type Parser<'a> = BigQuerySourceParser;
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn result_rows(&mut self) {
        let cquery = count_query(&self.query, &BigQueryDialect {})?;
        let job = self.client.job();
        let query_response = self.rt.block_on(job.query(
            self.project_id.as_str(),
            make_query_request_with_default_timeout(cquery.as_str()),
        ))?;
        let mut rs = ResultSet::new_from_query_response(query_response);
        rs.next_row();
        let nrows = rs
            .get_i64(0)?
            .ok_or_else(|| anyhow!("cannot get row number"))?;
        self.nrows = nrows as usize;
    }

    #[throws(BigQuerySourceError)]
    fn parser(&mut self) -> Self::Parser<'_> {
        let job = self.client.job();
        let qry = self.rt.block_on(job.query(
            self.project_id.as_str(),
            make_query_request_with_default_timeout(self.query.as_str()),
        ))?;
        let job_info = qry
            .job_reference
            .as_ref()
            .ok_or_else(|| anyhow!("job_reference is none"))?;
        let params = GetQueryResultsParameters {
            format_options: None,
            location: job_info.location.clone(),
            max_results: None,
            page_token: None,
            start_index: None,
            timeout_ms: Some(DEFAULT_TIMEOUT_MS),
        };
        let rs = self.rt.block_on(
            job.get_query_results(
                self.project_id.as_str(),
                job_info
                    .job_id
                    .as_ref()
                    .ok_or_else(|| anyhow!("job_id is none"))?
                    .as_str(),
                params,
            ),
        )?;
        BigQuerySourceParser::new(self.rt.clone(), self.client.clone(), rs, &self.schema)
    }

    fn nrows(&self) -> usize {
        self.nrows
    }

    fn ncols(&self) -> usize {
        self.ncols
    }
}

pub struct BigQuerySourceParser {
    rt: Arc<Runtime>,
    client: Arc<Client>,
    response: GetQueryResultsResponse,
    ncols: usize,
    current_col: usize,
    current_row: usize,
    nrows: Option<usize>,
}

impl<'a> BigQuerySourceParser {
    fn new(
        rt: Arc<Runtime>,
        client: Arc<Client>,
        response: GetQueryResultsResponse,
        schema: &[BigQueryTypeSystem],
    ) -> Self {
        Self {
            rt,
            client,
            response,
            ncols: schema.len(),
            current_row: 0,
            current_col: 0,
            nrows: None,
        }
    }

    #[throws(BigQuerySourceError)]
    fn next_loc(&mut self) -> (usize, usize) {
        let ret = (self.current_row, self.current_col);
        self.current_row += (self.current_col + 1) / self.ncols;
        self.current_col = (self.current_col + 1) % self.ncols;
        ret
    }
}

impl PartitionParser<'_> for BigQuerySourceParser {
    type TypeSystem = BigQueryTypeSystem;
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn fetch_next(&mut self) -> (usize, bool) {
        assert!(self.current_col == 0);
        match self.nrows {
            Some(total_rows) => (total_rows - self.current_row, true),
            None => {
                // Get all number of rows
                let total_rows = self
                    .response
                    .total_rows
                    .as_ref()
                    .ok_or_else(|| anyhow!("total_rows is none"))?
                    .parse::<usize>()?;
                self.nrows = Some(total_rows);
                (total_rows, true)
            }
        }
    }
}

macro_rules! impl_produce {
    ($($t: ty,)+) => {
        $(
            impl<'r> Produce<'r, $t> for BigQuerySourceParser {
                type Error = BigQuerySourceError;

                #[throws(BigQuerySourceError)]
                fn produce(&'r mut self) -> $t {
                    let (mut ridx, cidx) = self.next_loc()?;
                    if ridx == (self.response.rows.as_ref().ok_or_else(|| anyhow!("rows is none"))?.len()) {
                        let job = self.client.job();
                        let job_info = self.response.job_reference.as_ref().ok_or_else(|| anyhow!("job_reference is none"))?;
                        let params = GetQueryResultsParameters { format_options: None, location: job_info.location.clone(), max_results: None, page_token: self.response.page_token.clone(), start_index: None, timeout_ms: None };
                        self.response = self.rt.block_on(
                            job.get_query_results(
                                job_info.project_id.as_ref().ok_or_else(|| anyhow!("project_id is none"))?.as_str(),
                                job_info.job_id.as_ref().ok_or_else(|| anyhow!("job_id is none"))?.as_str(),
                                params,
                            ),
                        )?;
                        self.current_row = 0;
                        ridx = 0;
                    }
                    let rows = self.response.rows.as_ref().ok_or_else(|| anyhow!("rows is none"))?;
                    let columns = rows[ridx].columns.as_ref().ok_or_else(|| anyhow!("columns is none"))?;
                    let v = columns.get(cidx).ok_or_else(|| anyhow!("Table Cell is none"))?.value.as_ref().ok_or_else(|| anyhow!("value is none"))?;
                    let s = v
                        .as_str()
                        .ok_or_else(|| anyhow!("cannot get str from json value"))?;
                    s.parse()
                        .map_err(|_| {
                            ConnectorXError::cannot_produce::<$t>(Some(s.into()))
                        })?
                }
            }

            impl<'r> Produce<'r, Option<$t>> for BigQuerySourceParser {
                type Error = BigQuerySourceError;

                #[throws(BigQuerySourceError)]
                fn produce(&'r mut self) -> Option<$t> {
                    let (mut ridx, cidx) = self.next_loc()?;
                    if ridx == (self.response.rows.as_ref().ok_or_else(|| anyhow!("rows is none"))?.len()) {
                        let job = self.client.job();
                        let job_info = self.response.job_reference.as_ref().ok_or_else(|| anyhow!("job_reference is none"))?;
                        let params = GetQueryResultsParameters { format_options: None, location: job_info.location.clone(), max_results: None, page_token: self.response.page_token.clone(), start_index: None, timeout_ms: None };
                        self.response = self.rt.block_on(
                            job.get_query_results(
                                job_info.project_id.as_ref().ok_or_else(|| anyhow!("project_id is none"))?.as_str(),
                                job_info.job_id.as_ref().ok_or_else(|| anyhow!("job_id is none"))?.as_str(),
                                params,
                            ),
                        )?;
                        self.current_row = 0;
                        ridx = 0;
                    }
                    let rows = self.response.rows.as_ref().ok_or_else(|| anyhow!("rows is none"))?;
                    let columns = rows[ridx].columns.as_ref().ok_or_else(|| anyhow!("columns is none"))?;
                    match &columns.get(cidx).ok_or_else(|| anyhow!("Table Cell is none"))?.value {
                        None => None,
                        Some(v) => {
                            let s = v.as_str().ok_or_else(|| anyhow!("cannot get str from json value"))?;
                            Some(s.parse().map_err(|_| {
                            ConnectorXError::cannot_produce::<$t>(Some(s.into()))
                        })?)},
                    }
                }
            }
        )+
    };
}

impl_produce!(i64, f64, String,);

impl<'a> Produce<'_, bool> for BigQuerySourceParser {
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn produce(&mut self) -> bool {
        let (mut ridx, cidx) = self.next_loc()?;
        if ridx
            == (self
                .response
                .rows
                .as_ref()
                .ok_or_else(|| anyhow!("rows is none"))?
                .len())
        {
            let job = self.client.job();
            let job_info = self
                .response
                .job_reference
                .as_ref()
                .ok_or_else(|| anyhow!("job_reference is none"))?;
            let params = GetQueryResultsParameters {
                format_options: None,
                location: job_info.location.clone(),
                max_results: None,
                page_token: self.response.page_token.clone(),
                start_index: None,
                timeout_ms: None,
            };
            self.response = self.rt.block_on(
                job.get_query_results(
                    job_info
                        .project_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("project_id is none"))?
                        .as_str(),
                    job_info
                        .job_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("job_id is none"))?
                        .as_str(),
                    params,
                ),
            )?;
            self.current_row = 0;
            ridx = 0;
        }
        let rows = self
            .response
            .rows
            .as_ref()
            .ok_or_else(|| anyhow!("rows is none"))?;
        let columns = rows[ridx]
            .columns
            .as_ref()
            .ok_or_else(|| anyhow!("columns is none"))?;
        let v = columns
            .get(cidx)
            .ok_or_else(|| anyhow!("Table Cell is none"))?
            .value
            .as_ref()
            .ok_or_else(|| anyhow!("value is none"))?;
        let s = v
            .as_str()
            .ok_or_else(|| anyhow!("cannot get str from json value"))?;

        let ret = match s {
            "true" => true,
            "false" => false,
            _ => throw!(ConnectorXError::cannot_produce::<bool>(Some(s.into()))),
        };
        ret
    }
}

impl<'a> Produce<'_, Option<bool>> for BigQuerySourceParser {
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn produce(&mut self) -> Option<bool> {
        let (mut ridx, cidx) = self.next_loc()?;
        if ridx
            == (self
                .response
                .rows
                .as_ref()
                .ok_or_else(|| anyhow!("rows is none"))?
                .len())
        {
            let job = self.client.job();
            let job_info = self
                .response
                .job_reference
                .as_ref()
                .ok_or_else(|| anyhow!("job_reference is none"))?;
            let params = GetQueryResultsParameters {
                format_options: None,
                location: job_info.location.clone(),
                max_results: None,
                page_token: self.response.page_token.clone(),
                start_index: None,
                timeout_ms: None,
            };
            self.response = self.rt.block_on(
                job.get_query_results(
                    job_info
                        .project_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("project_id is none"))?
                        .as_str(),
                    job_info
                        .job_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("job_id is none"))?
                        .as_str(),
                    params,
                ),
            )?;
            self.current_row = 0;
            ridx = 0;
        }
        let rows = self
            .response
            .rows
            .as_ref()
            .ok_or_else(|| anyhow!("rows is none"))?;
        let columns = rows[ridx]
            .columns
            .as_ref()
            .ok_or_else(|| anyhow!("columns is none"))?;
        let ret = match &columns
            .get(cidx)
            .ok_or_else(|| anyhow!("Table Cell is none"))?
            .value
        {
            None => None,
            Some(v) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| anyhow!("cannot get str from json value"))?;
                match s {
                    "true" => Some(true),
                    "false" => Some(false),
                    _ => throw!(ConnectorXError::cannot_produce::<bool>(Some(s.into()))),
                }
            }
        };
        ret
    }
}

impl<'a> Produce<'_, NaiveDate> for BigQuerySourceParser {
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn produce(&mut self) -> NaiveDate {
        let (mut ridx, cidx) = self.next_loc()?;
        if ridx
            == (self
                .response
                .rows
                .as_ref()
                .ok_or_else(|| anyhow!("rows is none"))?
                .len())
        {
            let job = self.client.job();
            let job_info = self
                .response
                .job_reference
                .as_ref()
                .ok_or_else(|| anyhow!("job_reference is none"))?;
            let params = GetQueryResultsParameters {
                format_options: None,
                location: job_info.location.clone(),
                max_results: None,
                page_token: self.response.page_token.clone(),
                start_index: None,
                timeout_ms: None,
            };
            self.response = self.rt.block_on(
                job.get_query_results(
                    job_info
                        .project_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("project_id is none"))?
                        .as_str(),
                    job_info
                        .job_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("job_id is none"))?
                        .as_str(),
                    params,
                ),
            )?;
            self.current_row = 0;
            ridx = 0;
        }
        let rows = self
            .response
            .rows
            .as_ref()
            .ok_or_else(|| anyhow!("rows is none"))?;
        let columns = rows[ridx]
            .columns
            .as_ref()
            .ok_or_else(|| anyhow!("columns is none"))?;
        let v = columns
            .get(cidx)
            .ok_or_else(|| anyhow!("Table Cell is none"))?
            .value
            .as_ref()
            .ok_or_else(|| anyhow!("value is none"))?;
        let s = v
            .as_str()
            .ok_or_else(|| anyhow!("cannot get str from json value"))?;
        NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .map_err(|_| ConnectorXError::cannot_produce::<NaiveDate>(Some(s.into())))?
    }
}

impl<'a> Produce<'_, Option<NaiveDate>> for BigQuerySourceParser {
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn produce(&mut self) -> Option<NaiveDate> {
        let (mut ridx, cidx) = self.next_loc()?;
        if ridx
            == (self
                .response
                .rows
                .as_ref()
                .ok_or_else(|| anyhow!("rows is none"))?
                .len())
        {
            let job = self.client.job();
            let job_info = self
                .response
                .job_reference
                .as_ref()
                .ok_or_else(|| anyhow!("job_reference is none"))?;
            let params = GetQueryResultsParameters {
                format_options: None,
                location: job_info.location.clone(),
                max_results: None,
                page_token: self.response.page_token.clone(),
                start_index: None,
                timeout_ms: None,
            };
            self.response = self.rt.block_on(
                job.get_query_results(
                    job_info
                        .project_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("project_id is none"))?
                        .as_str(),
                    job_info
                        .job_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("job_id is none"))?
                        .as_str(),
                    params,
                ),
            )?;
            self.current_row = 0;
            ridx = 0;
        }
        let rows = self
            .response
            .rows
            .as_ref()
            .ok_or_else(|| anyhow!("rows is none"))?;
        let columns = rows[ridx]
            .columns
            .as_ref()
            .ok_or_else(|| anyhow!("columns is none"))?;
        match &columns
            .get(cidx)
            .ok_or_else(|| anyhow!("Table Cell is none"))?
            .value
        {
            None => None,
            Some(v) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| anyhow!("cannot get str from json value"))?;
                Some(
                    NaiveDate::parse_from_str(s, "%Y-%m-%d").map_err(|_| {
                        ConnectorXError::cannot_produce::<NaiveDate>(Some(s.into()))
                    })?,
                )
            }
        }
    }
}

impl<'a> Produce<'_, NaiveDateTime> for BigQuerySourceParser {
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn produce(&mut self) -> NaiveDateTime {
        let (mut ridx, cidx) = self.next_loc()?;
        if ridx
            == (self
                .response
                .rows
                .as_ref()
                .ok_or_else(|| anyhow!("rows is none"))?
                .len())
        {
            let job = self.client.job();
            let job_info = self
                .response
                .job_reference
                .as_ref()
                .ok_or_else(|| anyhow!("job_reference is none"))?;
            let params = GetQueryResultsParameters {
                format_options: None,
                location: job_info.location.clone(),
                max_results: None,
                page_token: self.response.page_token.clone(),
                start_index: None,
                timeout_ms: None,
            };
            self.response = self.rt.block_on(
                job.get_query_results(
                    job_info
                        .project_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("project_id is none"))?
                        .as_str(),
                    job_info
                        .job_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("job_id is none"))?
                        .as_str(),
                    params,
                ),
            )?;
            self.current_row = 0;
            ridx = 0;
        }
        let rows = self
            .response
            .rows
            .as_ref()
            .ok_or_else(|| anyhow!("rows is none"))?;
        let columns = rows[ridx]
            .columns
            .as_ref()
            .ok_or_else(|| anyhow!("columns is none"))?;
        let v = columns
            .get(cidx)
            .ok_or_else(|| anyhow!("Table Cell is none"))?
            .value
            .as_ref()
            .ok_or_else(|| anyhow!("value is none"))?;
        let s = v
            .as_str()
            .ok_or_else(|| anyhow!("cannot get str from json value"))?;
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
            .map_err(|_| ConnectorXError::cannot_produce::<NaiveDateTime>(Some(s.into())))?
    }
}

impl<'a> Produce<'_, Option<NaiveDateTime>> for BigQuerySourceParser {
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn produce(&mut self) -> Option<NaiveDateTime> {
        let (mut ridx, cidx) = self.next_loc()?;
        if ridx
            == (self
                .response
                .rows
                .as_ref()
                .ok_or_else(|| anyhow!("rows is none"))?
                .len())
        {
            let job = self.client.job();
            let job_info = self
                .response
                .job_reference
                .as_ref()
                .ok_or_else(|| anyhow!("job_reference is none"))?;
            let params = GetQueryResultsParameters {
                format_options: None,
                location: job_info.location.clone(),
                max_results: None,
                page_token: self.response.page_token.clone(),
                start_index: None,
                timeout_ms: None,
            };
            self.response = self.rt.block_on(
                job.get_query_results(
                    job_info
                        .project_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("project_id is none"))?
                        .as_str(),
                    job_info
                        .job_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("job_id is none"))?
                        .as_str(),
                    params,
                ),
            )?;
            self.current_row = 0;
            ridx = 0;
        }
        let rows = self
            .response
            .rows
            .as_ref()
            .ok_or_else(|| anyhow!("rows is none"))?;
        let columns = rows[ridx]
            .columns
            .as_ref()
            .ok_or_else(|| anyhow!("columns is none"))?;
        match &columns
            .get(cidx)
            .ok_or_else(|| anyhow!("Table Cell is none"))?
            .value
        {
            None => None,
            Some(v) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| anyhow!("cannot get str from json value"))?;
                Some(
                    NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S").map_err(|_| {
                        ConnectorXError::cannot_produce::<NaiveDateTime>(Some(s.into()))
                    })?,
                )
            }
        }
    }
}

impl<'a> Produce<'_, NaiveTime> for BigQuerySourceParser {
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn produce(&mut self) -> NaiveTime {
        let (mut ridx, cidx) = self.next_loc()?;
        if ridx
            == (self
                .response
                .rows
                .as_ref()
                .ok_or_else(|| anyhow!("rows is none"))?
                .len())
        {
            let job = self.client.job();
            let job_info = self
                .response
                .job_reference
                .as_ref()
                .ok_or_else(|| anyhow!("job_reference is none"))?;
            let params = GetQueryResultsParameters {
                format_options: None,
                location: job_info.location.clone(),
                max_results: None,
                page_token: self.response.page_token.clone(),
                start_index: None,
                timeout_ms: None,
            };
            self.response = self.rt.block_on(
                job.get_query_results(
                    job_info
                        .project_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("project_id is none"))?
                        .as_str(),
                    job_info
                        .job_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("job_id is none"))?
                        .as_str(),
                    params,
                ),
            )?;
            self.current_row = 0;
            ridx = 0;
        }
        let rows = self
            .response
            .rows
            .as_ref()
            .ok_or_else(|| anyhow!("rows is none"))?;
        let columns = rows[ridx]
            .columns
            .as_ref()
            .ok_or_else(|| anyhow!("columns is none"))?;
        let v = columns
            .get(cidx)
            .ok_or_else(|| anyhow!("Table Cell is none"))?
            .value
            .as_ref()
            .ok_or_else(|| anyhow!("value is none"))?;
        let s = v
            .as_str()
            .ok_or_else(|| anyhow!("cannot get str from json value"))?;
        NaiveTime::parse_from_str(s, "%H:%M:%S")
            .map_err(|_| ConnectorXError::cannot_produce::<NaiveTime>(Some(s.into())))?
    }
}

impl<'a> Produce<'_, Option<NaiveTime>> for BigQuerySourceParser {
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn produce(&mut self) -> Option<NaiveTime> {
        let (mut ridx, cidx) = self.next_loc()?;
        if ridx
            == (self
                .response
                .rows
                .as_ref()
                .ok_or_else(|| anyhow!("rows is none"))?
                .len())
        {
            let job = self.client.job();
            let job_info = self
                .response
                .job_reference
                .as_ref()
                .ok_or_else(|| anyhow!("job_reference is none"))?;
            let params = GetQueryResultsParameters {
                format_options: None,
                location: job_info.location.clone(),
                max_results: None,
                page_token: self.response.page_token.clone(),
                start_index: None,
                timeout_ms: None,
            };
            self.response = self.rt.block_on(
                job.get_query_results(
                    job_info
                        .project_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("project_id is none"))?
                        .as_str(),
                    job_info
                        .job_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("job_id is none"))?
                        .as_str(),
                    params,
                ),
            )?;
            self.current_row = 0;
            ridx = 0;
        }
        let rows = self
            .response
            .rows
            .as_ref()
            .ok_or_else(|| anyhow!("rows is none"))?;
        let columns = rows[ridx]
            .columns
            .as_ref()
            .ok_or_else(|| anyhow!("columns is none"))?;
        match &columns
            .get(cidx)
            .ok_or_else(|| anyhow!("Table Cell is none"))?
            .value
        {
            None => None,
            Some(v) => {
                let s = v
                    .as_str()
                    .ok_or_else(|| anyhow!("cannot get str from json value"))?;
                Some(
                    NaiveTime::parse_from_str(s, "%H:%M:%S").map_err(|_| {
                        ConnectorXError::cannot_produce::<NaiveTime>(Some(s.into()))
                    })?,
                )
            }
        }
    }
}

impl<'a> Produce<'_, DateTime<Utc>> for BigQuerySourceParser {
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn produce(&mut self) -> DateTime<Utc> {
        let (mut ridx, cidx) = self.next_loc()?;
        if ridx
            == (self
                .response
                .rows
                .as_ref()
                .ok_or_else(|| anyhow!("rows is none"))?
                .len())
        {
            let job = self.client.job();
            let job_info = self
                .response
                .job_reference
                .as_ref()
                .ok_or_else(|| anyhow!("job_reference is none"))?;
            let params = GetQueryResultsParameters {
                format_options: None,
                location: job_info.location.clone(),
                max_results: None,
                page_token: self.response.page_token.clone(),
                start_index: None,
                timeout_ms: None,
            };
            self.response = self.rt.block_on(
                job.get_query_results(
                    job_info
                        .project_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("project_id is none"))?
                        .as_str(),
                    job_info
                        .job_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("job_id is none"))?
                        .as_str(),
                    params,
                ),
            )?;
            self.current_row = 0;
            ridx = 0;
        }
        let rows = self
            .response
            .rows
            .as_ref()
            .ok_or_else(|| anyhow!("rows is none"))?;
        let columns = rows[ridx]
            .columns
            .as_ref()
            .ok_or_else(|| anyhow!("columns is none"))?;
        let v = columns
            .get(cidx)
            .ok_or_else(|| anyhow!("Table Cell is none"))?
            .value
            .as_ref()
            .ok_or_else(|| anyhow!("value is none"))?;
        let timestamp_ns = (v
            .as_str()
            .ok_or_else(|| anyhow!("cannot get str from json value"))?
            .parse::<f64>()?
            * 1e9) as i64;
        let secs = timestamp_ns / 1000000000;
        let nsecs = (timestamp_ns % 1000000000) as u32;
        DateTime::from_timestamp(secs, nsecs)
            .unwrap_or_else(|| panic!("out of range number: {secs} {nsecs}"))
    }
}

impl<'a> Produce<'_, Option<DateTime<Utc>>> for BigQuerySourceParser {
    type Error = BigQuerySourceError;

    #[throws(BigQuerySourceError)]
    fn produce(&mut self) -> Option<DateTime<Utc>> {
        let (mut ridx, cidx) = self.next_loc()?;
        if ridx
            == (self
                .response
                .rows
                .as_ref()
                .ok_or_else(|| anyhow!("rows is none"))?
                .len())
        {
            let job = self.client.job();
            let job_info = self
                .response
                .job_reference
                .as_ref()
                .ok_or_else(|| anyhow!("job_reference is none"))?;
            let params = GetQueryResultsParameters {
                format_options: None,
                location: job_info.location.clone(),
                max_results: None,
                page_token: self.response.page_token.clone(),
                start_index: None,
                timeout_ms: None,
            };
            self.response = self.rt.block_on(
                job.get_query_results(
                    job_info
                        .project_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("project_id is none"))?
                        .as_str(),
                    job_info
                        .job_id
                        .as_ref()
                        .ok_or_else(|| anyhow!("job_id is none"))?
                        .as_str(),
                    params,
                ),
            )?;
            self.current_row = 0;
            ridx = 0;
        }
        let rows = self
            .response
            .rows
            .as_ref()
            .ok_or_else(|| anyhow!("rows is none"))?;
        let columns = rows[ridx]
            .columns
            .as_ref()
            .ok_or_else(|| anyhow!("columns is none"))?;
        match &columns
            .get(cidx)
            .ok_or_else(|| anyhow!("Table Cell is none"))?
            .value
        {
            None => None,
            Some(v) => {
                let timestamp_ns = (v
                    .as_str()
                    .ok_or_else(|| anyhow!("cannot get str from json value"))?
                    .parse::<f64>()?
                    * 1e9) as i64;
                let secs = timestamp_ns / 1000000000;
                let nsecs = (timestamp_ns % 1000000000) as u32;
                DateTime::from_timestamp(secs, nsecs)
            }
        }
    }
}
