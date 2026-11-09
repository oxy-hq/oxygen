use reqwest::Client;

use crate::adapters::secrets::SecretsManager;
use crate::config::model::DOMO as DOMOConfig;
use crate::connector::Engine;
use crate::connector::domo::ai::DOMOAI;
use crate::connector::domo::dataset::DOMODataset;
use crate::connector::domo::query::DOMOQuery;
use crate::connector::domo::types::ExecuteQueryRequest;
use oxy_shared::errors::OxyError;

mod ai;
mod dataset;
mod query;
pub mod types;

#[derive(Debug)]
pub struct DOMO {
    client: Client,
    base_url: String,
    dataset_id: String,
}

impl DOMO {
    fn new(client: Client, base_url: String, dataset_id: String) -> Self {
        Self {
            client,
            base_url,
            dataset_id,
        }
    }

    pub async fn from_config(
        secrets_manager: SecretsManager,
        config: DOMOConfig,
    ) -> Result<Self, OxyError> {
        let mut headers = reqwest::header::HeaderMap::new();
        let domo_auth_token = secrets_manager
            .resolve_secret(&config.developer_token_var)
            .await?
            .ok_or(OxyError::ConfigurationError(format!(
                "DOMO token variable '{}' not found or is empty",
                config.developer_token_var
            )))?;
        let mut auth_value =
            reqwest::header::HeaderValue::from_str(&domo_auth_token).map_err(|err| {
                OxyError::ConfigurationError(format!(
                    "Failed to create header value from DOMO token: {}",
                    err
                ))
            })?;
        auth_value.set_sensitive(true);
        headers.insert("X-DOMO-Developer-Token", auth_value);

        let client = oxy_shared::http::builder()
            .default_headers(headers)
            .build()
            .map_err(|err| {
                OxyError::ConfigurationError(format!("Failed to build HTTP client: {}", err))
            })?;
        let base_url = format!("https://{}.domo.com/api", config.instance);
        Ok(Self::new(client, base_url, config.dataset_id.to_string()))
    }

    fn post(&self, endpoint: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, endpoint);
        self.client
            .post(&url)
            .header("Content-Type", "application/json")
    }

    fn get(&self, endpoint: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, endpoint);
        self.client.get(&url)
    }

    fn query(&'_ self) -> DOMOQuery<'_> {
        DOMOQuery::new(self, &self.dataset_id)
    }

    pub fn dataset(&self) -> DOMODataset<'_> {
        DOMODataset::new(self)
    }

    pub fn ai(&self) -> DOMOAI<'_> {
        DOMOAI::new(self)
    }
}

impl Engine for DOMO {
    async fn run_query_with_limit(
        &self,
        query: &str,
        _dry_run_limit: Option<u64>,
    ) -> Result<
        (
            Vec<arrow::record_batch::RecordBatch>,
            arrow::datatypes::SchemaRef,
        ),
        OxyError,
    > {
        tracing::debug!("🔍 DOMO query: {}", query);
        let sql_request = ExecuteQueryRequest {
            sql: query.to_string(),
        };
        let json_response = self.query().execute(&sql_request).await?;
        self.query().to_record_batches(json_response)
    }
}
