//! Looker API client implementation

use std::time::Duration;

use reqwest::{Client, RequestBuilder, Response};
use tokio::time::Instant;
use tracing::{debug, warn};

use crate::error::LookerError;
use crate::models::{
    InlineQueryRequest, LoginResponse, LookmlModel, LookmlModelExplore, Query, QueryResponse,
};

/// Buffer time before token expiration to trigger refresh (30 seconds).
const TOKEN_REFRESH_BUFFER_SECS: u64 = 30;

/// Configuration for Looker API authentication.
///
/// This struct holds the credentials and base URL needed to authenticate
/// with the Looker API using OAuth 2.0-like flow.
#[derive(Debug, Clone)]
pub struct LookerAuthConfig {
    /// The base URL of the Looker instance (e.g., https://your.looker.com:19999)
    pub base_url: String,
    /// The client ID for API authentication
    pub client_id: String,
    /// The client secret for API authentication
    pub client_secret: String,
}

/// Represents an access token obtained from Looker API authentication.
///
/// The token is short-lived (default: 1 hour) and should be refreshed
/// when it expires.
#[derive(Debug)]
pub struct AccessToken {
    /// The access token string used for API authorization
    pub token: String,
    /// The point in time when this token expires
    pub expires_at: Instant,
}

impl AccessToken {
    /// Returns true if the token is expired or will expire soon.
    pub fn is_expired(&self) -> bool {
        Instant::now() >= self.expires_at - Duration::from_secs(TOKEN_REFRESH_BUFFER_SECS)
    }
}

/// Looker API client for interacting with Looker services.
///
/// This client handles authentication automatically, refreshing the access token
/// when it expires.
///
/// # Example
///
/// ```ignore
/// let config = LookerAuthConfig {
///     base_url: "https://your.looker.com:19999".to_string(),
///     client_id: "your_client_id".to_string(),
///     client_secret: "your_client_secret".to_string(),
/// };
///
/// let mut client = LookerApiClient::new(config)?;
/// // The client will automatically authenticate when needed
/// ```
#[derive(Debug)]
pub struct LookerApiClient {
    config: LookerAuthConfig,
    access_token: Option<AccessToken>,
    http_client: Client,
}

impl LookerApiClient {
    /// Creates a new Looker API client with the given configuration.
    pub fn new(config: LookerAuthConfig) -> Result<Self, LookerError> {
        let http_client = oxy_shared::http::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| LookerError::ConfigError {
                message: format!("Failed to create HTTP client: {}", e),
            })?;

        Ok(Self {
            config,
            access_token: None,
            http_client,
        })
    }

    /// Authenticates with the Looker API and obtains an access token.
    ///
    /// This method sends a POST request to `/api/4.0/login` with the client credentials
    /// and stores the returned access token for subsequent requests.
    pub async fn authenticate(&mut self) -> Result<(), LookerError> {
        let login_url = format!(
            "{}/api/4.0/login",
            self.config.base_url.trim_end_matches('/')
        );

        let response = self
            .http_client
            .post(&login_url)
            .form(&[
                ("client_id", &self.config.client_id),
                ("client_secret", &self.config.client_secret),
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let message = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            return Err(if status == 401 || status == 403 {
                LookerError::AuthenticationError {
                    message: format!("Invalid credentials: {}", message),
                }
            } else {
                LookerError::ApiError { status, message }
            });
        }

        let login_response: LoginResponse = response.json().await?;

        // Calculate expiration time from now + expires_in seconds
        let expires_at = Instant::now() + Duration::from_secs(login_response.expires_in);

        self.access_token = Some(AccessToken {
            token: login_response.access_token,
            expires_at,
        });

        Ok(())
    }

    /// Ensures the client has a valid access token, refreshing if necessary.
    ///
    /// This method checks if the current token is valid and not expired.
    /// If there's no token or it's expired, it automatically re-authenticates.
    ///
    /// Returns a reference to the valid access token string.
    pub async fn ensure_authenticated(&mut self) -> Result<&str, LookerError> {
        let needs_auth = match &self.access_token {
            None => true,
            Some(token) => token.is_expired(),
        };

        if needs_auth {
            self.authenticate().await?;
        }

        self.access_token
            .as_ref()
            .map(|t| t.token.as_str())
            .ok_or_else(|| LookerError::AuthenticationError {
                message: "No access token available after authentication".to_string(),
            })
    }

    /// Adds the Authorization header to a request builder.
    ///
    /// This is a helper method for building authenticated requests.
    /// The header format is: `Authorization: token {access_token}`
    fn add_auth_header(&self, request: RequestBuilder) -> Result<RequestBuilder, LookerError> {
        let token = self
            .access_token
            .as_ref()
            .ok_or_else(|| LookerError::AuthenticationError {
                message: "Not authenticated".to_string(),
            })?;

        Ok(request.header("Authorization", format!("token {}", token.token)))
    }

    /// Creates an authenticated GET request to the specified endpoint.
    ///
    /// Ensures authentication before building the request.
    pub async fn get(&mut self, endpoint: &str) -> Result<RequestBuilder, LookerError> {
        self.ensure_authenticated().await?;

        let url = format!(
            "{}/api/4.0/{}",
            self.config.base_url.trim_end_matches('/'),
            endpoint.trim_start_matches('/')
        );

        self.add_auth_header(self.http_client.get(&url))
    }

    /// Creates an authenticated POST request to the specified endpoint.
    ///
    /// Ensures authentication before building the request.
    pub async fn post(&mut self, endpoint: &str) -> Result<RequestBuilder, LookerError> {
        self.ensure_authenticated().await?;

        let url = format!(
            "{}/api/4.0/{}",
            self.config.base_url.trim_end_matches('/'),
            endpoint.trim_start_matches('/')
        );

        self.add_auth_header(self.http_client.post(&url))
    }

    /// Returns the base URL of the Looker instance.
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// Handles an HTTP response, converting errors to `LookerError`.
    ///
    /// This method checks for:
    /// - Rate limiting (429 status) with retry-after header parsing
    /// - Authentication errors (401, 403)
    /// - Not found errors (404)
    /// - Other API errors
    async fn handle_response(&self, response: Response) -> Result<Response, LookerError> {
        let status = response.status();

        if status.is_success() {
            return Ok(response);
        }

        let status_code = status.as_u16();

        // Handle rate limiting (429 Too Many Requests)
        if status_code == 429 {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|h| h.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(60); // Default to 60 seconds if header is missing

            warn!(
                retry_after_seconds = retry_after,
                "Looker API rate limit exceeded"
            );

            return Err(LookerError::RateLimitError {
                retry_after_seconds: retry_after,
            });
        }

        let message = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());

        if status_code == 401 || status_code == 403 {
            return Err(LookerError::AuthenticationError {
                message: format!("Authentication failed ({}): {}", status_code, message),
            });
        }

        // Handle not found errors
        if status_code == 404 {
            return Err(LookerError::NotFoundError { resource: message });
        }

        Err(LookerError::ApiError {
            status: status_code,
            message,
        })
    }

    /// Lists all LookML models available in the Looker instance.
    ///
    /// Retrieves model metadata including their explores (topics).
    ///
    /// # Example
    ///
    /// ```ignore
    /// let models = client.list_models().await?;
    /// for model in models {
    ///     println!("Model: {} ({} explores)", model.name, model.explores.len());
    /// }
    /// ```
    pub async fn list_models(&mut self) -> Result<Vec<LookmlModel>, LookerError> {
        debug!("Fetching LookML models from Looker API");

        let request = self.get("lookml_models").await?;
        let response = request.send().await?;
        let response = self.handle_response(response).await?;
        let models: Vec<LookmlModel> = response.json().await?;

        debug!(model_count = models.len(), "Retrieved LookML models");
        Ok(models)
    }

    /// Retrieves detailed metadata for a specific explore within a model.
    ///
    /// This includes all fields (dimensions, measures, filters, parameters),
    /// SQL table name, and other explore configuration.
    ///
    /// # Arguments
    ///
    /// * `model` - The name of the LookML model
    /// * `explore` - The name of the explore within the model
    ///
    /// # Example
    ///
    /// ```ignore
    /// let explore = client.get_explore("ecommerce", "orders").await?;
    /// if let Some(fields) = explore.fields {
    ///     println!("Dimensions: {}", fields.dimensions.len());
    ///     println!("Measures: {}", fields.measures.len());
    /// }
    /// ```
    pub async fn get_explore(
        &mut self,
        model: &str,
        explore: &str,
    ) -> Result<LookmlModelExplore, LookerError> {
        debug!(
            model = model,
            explore = explore,
            "Fetching explore metadata"
        );

        let endpoint = format!("lookml_models/{}/explores/{}", model, explore);
        let request = self.get(&endpoint).await?;
        let response = request.send().await?;
        let response = self.handle_response(response).await?;
        let explore_metadata: LookmlModelExplore = response.json().await?;

        debug!(
            model = model,
            explore = explore,
            field_count = explore_metadata
                .fields
                .as_ref()
                .map(|f| f.dimensions.len() + f.measures.len())
                .unwrap_or(0),
            "Retrieved explore metadata"
        );

        Ok(explore_metadata)
    }

    // Query Endpoints

    /// Runs an inline query and returns the results.
    ///
    /// This method executes a query directly without creating a saved query first.
    /// It's the most common way to run ad-hoc queries.
    ///
    /// # Arguments
    ///
    /// * `query` - The query parameters including model, view, fields, filters, etc.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let query = InlineQueryRequest {
    ///     model: "ecommerce".to_string(),
    ///     view: "orders".to_string(),
    ///     fields: vec!["orders.id".to_string(), "orders.total".to_string()],
    ///     filters: Some(HashMap::from([
    ///         ("orders.created_date".to_string(), "last 30 days".to_string())
    ///     ])),
    ///     ..Default::default()
    /// };
    ///
    /// let response = client.run_inline_query(query).await?;
    /// println!("Got {} rows", response.data.len());
    /// ```
    pub async fn run_inline_query(
        &mut self,
        query: InlineQueryRequest,
    ) -> Result<QueryResponse, LookerError> {
        debug!(
            model = query.model,
            view = query.view,
            field_count = query.fields.len(),
            "Running inline query"
        );

        let request = self.post("queries/run/json_detail").await?;
        let response = request.json(&query).send().await?;
        let response = self.handle_response(response).await?;

        let query_response = self.parse_query_response(response).await?;

        debug!(
            row_count = query_response.data.len(),
            "Inline query completed"
        );

        Ok(query_response)
    }

    /// Creates a saved query without executing it.
    ///
    /// This is useful when you want to save a query for later execution,
    /// or when you need to get the query ID for other operations.
    ///
    /// # Arguments
    ///
    /// * `query` - The query parameters to save
    ///
    /// # Returns
    ///
    /// A `Query` object containing the query ID and configuration.
    ///
    /// # Example
    ///
    /// ```ignore
    /// let query_request = InlineQueryRequest {
    ///     model: "ecommerce".to_string(),
    ///     view: "orders".to_string(),
    ///     fields: vec!["orders.id".to_string()],
    ///     ..Default::default()
    /// };
    ///
    /// let saved_query = client.create_query(query_request).await?;
    /// println!("Created query with ID: {}", saved_query.id);
    ///
    /// // Later, run the query
    /// let results = client.run_query(saved_query.id, ResultFormat::Json).await?;
    /// ```
    pub async fn create_query(&mut self, query: InlineQueryRequest) -> Result<Query, LookerError> {
        debug!(
            model = query.model,
            view = query.view,
            field_count = query.fields.len(),
            "Creating query"
        );

        let request = self.post("queries").await?;
        let response = request.json(&query).send().await?;
        let response = self.handle_response(response).await?;

        let saved_query: Query = response.json().await.map_err(|e| LookerError::QueryError {
            message: format!("Failed to parse query response: {}", e),
        })?;

        debug!(query_id = saved_query.id, "Query created");

        Ok(saved_query)
    }

    /// Runs a previously saved query by its ID.
    ///
    /// # Arguments
    ///
    /// * `query_id` - The ID of the saved query to execute
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Run a query that was created earlier
    /// let response = client.run_query(12345).await?;
    /// for row in response.data {
    ///     println!("{:?}", row);
    /// }
    /// ```
    pub async fn run_query(&mut self, query_id: i64) -> Result<QueryResponse, LookerError> {
        debug!(query_id = query_id, "Running saved query");

        let endpoint = format!("queries/{}/run/json_detail", query_id);
        let request = self.get(&endpoint).await?;
        let response = request.send().await?;
        let response = self.handle_response(response).await?;

        let query_response = self.parse_query_response(response).await?;

        debug!(
            query_id = query_id,
            row_count = query_response.data.len(),
            "Saved query completed"
        );

        Ok(query_response)
    }

    /// Runs an inline query and returns the generated SQL string.
    ///
    /// This method calls Looker's `queries/run/sql` endpoint which returns the SQL
    /// that Looker would execute, without running the actual data query.
    pub async fn run_inline_query_sql(
        &mut self,
        query: InlineQueryRequest,
    ) -> Result<String, LookerError> {
        debug!(
            model = query.model,
            view = query.view,
            field_count = query.fields.len(),
            "Running inline query for SQL generation"
        );

        let request = self.post("queries/run/sql").await?;
        let response = request.json(&query).send().await?;
        let response = self.handle_response(response).await?;

        let sql = response.text().await.map_err(|e| LookerError::QueryError {
            message: format!("Failed to read SQL response: {}", e),
        })?;

        debug!("SQL generation completed");
        Ok(sql)
    }

    async fn parse_query_response(&self, response: Response) -> Result<QueryResponse, LookerError> {
        let response_text = response.text().await.map_err(|e| LookerError::QueryError {
            message: format!("Failed to read response: {}", e),
        })?;

        let parsed: serde_json::Value =
            serde_json::from_str(&response_text).map_err(|e| LookerError::QueryError {
                message: format!("Failed to parse JSON detail response: {}", e),
            })?;

        let data = if let Some(data_array) = parsed.get("data").and_then(|d| d.as_array()) {
            data_array
                .iter()
                .filter_map(|row| {
                    row.as_object().map(|obj| {
                        obj.iter()
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect::<std::collections::HashMap<String, serde_json::Value>>()
                    })
                })
                .collect()
        } else {
            Vec::new()
        };

        let fields = if let Some(fields_obj) = parsed.get("fields") {
            serde_json::from_value(fields_obj.clone()).unwrap_or_default()
        } else {
            std::collections::HashMap::new()
        };

        let sql = parsed
            .get("sql")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());

        Ok(QueryResponse { data, fields, sql })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_access_token_is_expired() {
        // Token that expires in 10 seconds (less than buffer) should be considered expired
        let token = AccessToken {
            token: "test".to_string(),
            expires_at: Instant::now() + Duration::from_secs(10),
        };
        assert!(token.is_expired());

        // Token that expires in 60 seconds should not be expired
        let token = AccessToken {
            token: "test".to_string(),
            expires_at: Instant::now() + Duration::from_secs(60),
        };
        assert!(!token.is_expired());
    }

    #[test]
    fn test_looker_auth_config() {
        let config = LookerAuthConfig {
            base_url: "https://example.looker.com:19999".to_string(),
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
        };

        assert_eq!(config.base_url, "https://example.looker.com:19999");
        assert_eq!(config.client_id, "test_id");
        assert_eq!(config.client_secret, "test_secret");
    }

    #[test]
    fn test_client_creation() {
        let config = LookerAuthConfig {
            base_url: "https://example.looker.com:19999".to_string(),
            client_id: "test_id".to_string(),
            client_secret: "test_secret".to_string(),
        };

        let client = LookerApiClient::new(config).unwrap();
        assert_eq!(client.base_url(), "https://example.looker.com:19999");
        assert!(client.access_token.is_none());
    }

    #[test]
    fn test_rate_limit_error_properties() {
        let error = LookerError::RateLimitError {
            retry_after_seconds: 30,
        };
        assert!(error.is_temporary());
        assert_eq!(error.retry_delay_seconds(), Some(30));
        assert!(error.user_friendly_message().contains("30 seconds"));
    }

    #[test]
    fn test_authentication_error_properties() {
        let error = LookerError::AuthenticationError {
            message: "Invalid credentials".to_string(),
        };
        assert!(!error.is_temporary());
        assert_eq!(error.retry_delay_seconds(), None);
        assert!(
            error
                .user_friendly_message()
                .contains("check your credentials")
        );
    }

    #[test]
    fn test_not_found_error_properties() {
        let error = LookerError::NotFoundError {
            resource: "model/explore".to_string(),
        };
        assert!(!error.is_temporary());
        assert_eq!(error.retry_delay_seconds(), None);
        assert!(error.user_friendly_message().contains("not found"));
    }

    #[test]
    fn test_api_error_properties() {
        let error = LookerError::ApiError {
            status: 500,
            message: "Internal server error".to_string(),
        };
        assert!(!error.is_temporary());
        assert_eq!(error.retry_delay_seconds(), None);
        assert!(error.user_friendly_message().contains("HTTP 500"));
    }

    #[test]
    fn test_connection_error_properties() {
        let error = LookerError::ConnectionError {
            message: "Connection refused".to_string(),
        };
        assert!(error.is_temporary());
        assert_eq!(error.retry_delay_seconds(), Some(5));
        assert!(error.user_friendly_message().contains("network connection"));
    }

    #[test]
    fn test_inline_query_request_creation() {
        let query = InlineQueryRequest {
            model: "ecommerce".to_string(),
            view: "orders".to_string(),
            fields: vec!["orders.id".to_string(), "orders.total".to_string()],
            filters: None,
            filter_expression: None,
            sorts: Some(vec!["orders.created_date desc".to_string()]),
            limit: Some(100),
            query_timezone: None,
            pivots: None,
            fill_fields: None,
        };

        assert_eq!(query.model, "ecommerce");
        assert_eq!(query.view, "orders");
        assert_eq!(query.fields.len(), 2);
        assert_eq!(query.limit, Some(100));
    }
}
