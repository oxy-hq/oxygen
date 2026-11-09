//! Looker API semantic engine implementation.
//!
//! Executes queries via Looker's `/api/4.0/queries/run/json` endpoint.
//!
//! # Configuration
//!
//! ```yaml
//! semantic_engine:
//!   vendor: looker
//!   base_url: https://myco.looker.com
//!   client_id: "${LOOKER_CLIENT_ID}"
//!   client_secret: "${LOOKER_CLIENT_SECRET}"
//! ```
//!
//! # Token management
//!
//! Access tokens are cached for the lifetime of the engine (Looker tokens are
//! valid for 1 hour). On 401, the token is refreshed once and the request
//! retried. A second 401 is returned as `EngineError::Transport`.

use agentic_core::result::{CellValue, QueryResult, QueryRow};
use async_trait::async_trait;
use reqwest::Client;
use tokio::sync::Mutex;

use super::translate::looker_translate;
use super::{EngineError, SemanticEngine, TranslationContext, VendorQuery};
use crate::types::AnalyticsIntent;

/// Looker API engine client.
pub struct LookerEngine {
    base_url: String,
    client_id: String,
    client_secret: String,
    client: Client,
    /// Cached access token (None until first use / after expiry).
    token: Mutex<Option<String>>,
}

impl LookerEngine {
    pub fn new(base_url: String, client_id: String, client_secret: String) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            client_id,
            client_secret,
            // `builder()`, not `client()`: a query runs as long as the engine
            // takes, so no overall timeout; the shared client is the fallback a
            // failed build (TLS init) would also have panicked on.
            client: oxy_shared::http::builder()
                .build()
                .unwrap_or_else(|_| oxy_shared::http::client()),
            token: Mutex::new(None),
        }
    }

    async fn authenticate(&self) -> Result<String, EngineError> {
        let url = format!("{}/api/4.0/login", self.base_url);
        let resp = self
            .client
            .post(&url)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
            ])
            .send()
            .await
            .map_err(|e| EngineError::Transport(format!("Looker login failed: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(EngineError::EngineUnreachable(format!(
                "Looker /api/4.0/login returned {status}: {body}"
            )));
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| EngineError::Transport(format!("Looker login parse error: {e}")))?;

        body.get("access_token")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                EngineError::EngineUnreachable("Looker login response missing access_token".into())
            })
    }

    async fn get_token(&self) -> Result<String, EngineError> {
        let cached = self.token.lock().await.clone();
        if let Some(tok) = cached {
            return Ok(tok);
        }
        let tok = self.authenticate().await?;
        *self.token.lock().await = Some(tok.clone());
        Ok(tok)
    }

    async fn run_query(
        &self,
        token: &str,
        query: &VendorQuery,
    ) -> Result<reqwest::Response, EngineError> {
        let url = format!("{}/api/4.0/queries/run/json", self.base_url);
        self.client
            .post(&url)
            .bearer_auth(token)
            .json(&query.payload)
            .send()
            .await
            .map_err(|e| EngineError::Transport(e.to_string()))
    }
}

#[async_trait]
impl SemanticEngine for LookerEngine {
    fn vendor_name(&self) -> &str {
        "looker"
    }

    fn translate(
        &self,
        ctx: &TranslationContext,
        intent: &AnalyticsIntent,
    ) -> Result<VendorQuery, EngineError> {
        looker_translate(ctx, intent)
    }

    async fn ping(&self) -> Result<(), EngineError> {
        // Authenticate and cache token — failure propagates as EngineUnreachable
        let tok = self.authenticate().await?;
        *self.token.lock().await = Some(tok);
        Ok(())
    }

    async fn execute(&self, query: &VendorQuery) -> Result<QueryResult, EngineError> {
        let token = self.get_token().await?;
        let resp = self.run_query(&token, query).await?;

        if resp.status().as_u16() == 401 {
            // Refresh token once and retry
            *self.token.lock().await = None;
            let new_token = self.get_token().await?;
            let resp2 = self.run_query(&new_token, query).await?;
            if resp2.status().as_u16() == 401 {
                return Err(EngineError::Transport(
                    "authentication failed after re-auth".to_string(),
                ));
            }
            return parse_looker_response(resp2).await;
        }

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(EngineError::ApiError { status, body });
        }

        parse_looker_response(resp).await
    }
}

async fn parse_looker_response(resp: reqwest::Response) -> Result<QueryResult, EngineError> {
    let data: Vec<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| EngineError::Transport(format!("Looker response parse error: {e}")))?;

    if data.is_empty() {
        return Ok(QueryResult {
            columns: vec![],
            rows: vec![],
            total_row_count: 0,
            truncated: false,
        });
    }

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
