//! Cube REST API semantic engine implementation.
//!
//! Executes queries via Cube's `/v1/load` endpoint.
//!
//! # Configuration
//!
//! ```yaml
//! semantic_engine:
//!   vendor: cube
//!   base_url: https://cube.example.com
//!   api_token: "${CUBE_API_TOKEN}"
//! ```

use agentic_core::result::{CellValue, QueryResult, QueryRow};
use async_trait::async_trait;
use reqwest::Client;

use super::translate::cube_translate;
use super::{EngineError, SemanticEngine, TranslationContext, VendorQuery};
use crate::types::AnalyticsIntent;

const MAX_CONTINUE_WAIT_RETRIES: usize = 3;
const CONTINUE_WAIT_DELAY_MS: u64 = 2000;

/// Cube REST API engine client.
pub struct CubeEngine {
    base_url: String,
    api_token: String,
    client: Client,
}

impl CubeEngine {
    pub fn new(base_url: String, api_token: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_token,
            // `builder()`, not `client()`: a query runs as long as the engine
            // takes, so no overall timeout; the shared client is the fallback a
            // failed build (TLS init) would also have panicked on.
            client: oxy_shared::http::builder()
                .build()
                .unwrap_or_else(|_| oxy_shared::http::client()),
        }
    }
}

#[async_trait]
impl SemanticEngine for CubeEngine {
    fn vendor_name(&self) -> &str {
        "cube"
    }

    fn translate(
        &self,
        ctx: &TranslationContext,
        intent: &AnalyticsIntent,
    ) -> Result<VendorQuery, EngineError> {
        cube_translate(ctx, intent)
    }

    async fn ping(&self) -> Result<(), EngineError> {
        let url = format!("{}/v1/meta", self.base_url);
        let resp = self
            .client
            .get(&url)
            .bearer_auth(&self.api_token)
            .send()
            .await
            .map_err(|e| EngineError::EngineUnreachable(e.to_string()))?;

        if resp.status().is_success() {
            Ok(())
        } else {
            Err(EngineError::EngineUnreachable(format!(
                "Cube /v1/meta returned status {}",
                resp.status()
            )))
        }
    }

    async fn execute(&self, query: &VendorQuery) -> Result<QueryResult, EngineError> {
        let url = format!("{}/v1/load", self.base_url);
        let mut retries = 0;

        loop {
            let resp = self
                .client
                .post(&url)
                .bearer_auth(&self.api_token)
                .json(&query.payload)
                .send()
                .await
                .map_err(|e| EngineError::Transport(e.to_string()))?;

            let status = resp.status();
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| EngineError::Transport(format!("failed to parse response: {e}")))?;

            // Cube returns { "error": "Continue wait" } with status 200 for long queries
            if body.get("error").and_then(|e| e.as_str()) == Some("Continue wait") {
                if retries >= MAX_CONTINUE_WAIT_RETRIES {
                    return Err(EngineError::Transport(
                        "Cube query timed out after max retries".to_string(),
                    ));
                }
                retries += 1;
                tokio::time::sleep(std::time::Duration::from_millis(
                    CONTINUE_WAIT_DELAY_MS * (1 << (retries - 1)),
                ))
                .await;
                continue;
            }

            if !status.is_success() {
                return Err(EngineError::ApiError {
                    status: status.as_u16(),
                    body: body.to_string(),
                });
            }

            return parse_cube_response(&body);
        }
    }
}

fn parse_cube_response(body: &serde_json::Value) -> Result<QueryResult, EngineError> {
    let data = body
        .get("data")
        .and_then(|d| d.as_array())
        .ok_or_else(|| EngineError::Transport(format!("unexpected Cube response shape: {body}")))?;

    if data.is_empty() {
        return Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            total_row_count: 0,
            truncated: false,
        });
    }

    // Column names from first row's keys
    let columns: Vec<String> = data[0]
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();

    let rows: Vec<QueryRow> = data
        .iter()
        .map(|row| {
            let cells = columns
                .iter()
                .map(|col| json_to_cell(row.get(col).unwrap_or(&serde_json::Value::Null)))
                .collect();
            QueryRow(cells)
        })
        .collect();

    let total = rows.len() as u64;
    Ok(QueryResult {
        columns,
        rows,
        total_row_count: total,
        truncated: false,
    })
}

fn json_to_cell(v: &serde_json::Value) -> CellValue {
    match v {
        serde_json::Value::Null => CellValue::Null,
        serde_json::Value::Bool(b) => CellValue::Text(b.to_string()),
        serde_json::Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                CellValue::Number(f)
            } else {
                CellValue::Text(n.to_string())
            }
        }
        serde_json::Value::String(s) => CellValue::Text(s.clone()),
        other => CellValue::Text(other.to_string()),
    }
}
