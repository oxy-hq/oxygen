use reqwest::{Client, RequestBuilder};
use serde::Deserialize;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

use crate::error::OmniError;
use crate::models::{ModelsResponse, QueryRequest, QueryResponse, TimeoutConfig, TopicResponse};
use crate::resilience::{RetryConfig, RetryPolicy, TimeoutWrapper};

/// Omni API client for interacting with Omni's semantic layer API
#[derive(Debug, Clone)]
pub struct OmniApiClient {
    base_url: String,
    api_token: String,
    client: Client,
    retry_policy: RetryPolicy,
    timeout_config: TimeoutConfig,
}

impl OmniApiClient {
    fn validate_client_config(base_url: &str, api_token: &str) -> Result<(), OmniError> {
        if base_url.is_empty() {
            return Err(OmniError::config_invalid(
                "base_url",
                "Base URL cannot be empty",
            ));
        }

        if api_token.is_empty() {
            return Err(OmniError::config_invalid(
                "api_token",
                "API token cannot be empty",
            ));
        }

        // Enhanced URL validation
        if base_url.len() > 2048 {
            return Err(OmniError::config_invalid(
                "base_url",
                "Base URL cannot exceed 2048 characters",
            ));
        }

        // Basic URL format validation
        if !base_url.starts_with("http://") && !base_url.starts_with("https://") {
            return Err(OmniError::config_invalid(
                "base_url",
                "Base URL must start with http:// or https://",
            ));
        }

        // Enhanced API token validation
        if api_token.len() < 10 {
            return Err(OmniError::config_invalid(
                "api_token",
                "API token appears to be too short (minimum 10 characters)",
            ));
        }

        if api_token.len() > 1024 {
            return Err(OmniError::config_invalid(
                "api_token",
                "API token cannot exceed 1024 characters",
            ));
        }

        if api_token.trim() != api_token {
            return Err(OmniError::config_invalid(
                "api_token",
                "API token cannot contain leading or trailing whitespace",
            ));
        }

        Ok(())
    }

    fn validate_timeout_config(timeout_config: &TimeoutConfig) -> Result<(), OmniError> {
        debug!(
            timeout_config = ?timeout_config,
            estimated_max_time_secs = timeout_config.estimate_max_polling_time().as_secs(),
            "Validating timeout configuration"
        );

        timeout_config.validate_config().map_err(|e| {
            warn!(
                timeout_config = ?timeout_config,
                validation_error = %e,
                "Timeout configuration failed basic validation"
            );
            OmniError::config_invalid(
                "timeout_config",
                &format!("Invalid timeout configuration: {}", e),
            )
        })?;

        timeout_config.validate_consistency().map_err(|e| {
            warn!(
                timeout_config = ?timeout_config,
                consistency_error = %e,
                "Timeout configuration failed consistency validation"
            );
            OmniError::config_invalid("timeout_config", &e)
        })?;

        // Additional validation for extreme values
        if timeout_config.max_polling_attempts > 200 {
            warn!(
                max_polling_attempts = timeout_config.max_polling_attempts,
                "Timeout configuration has excessive polling attempts"
            );
            return Err(OmniError::config_invalid(
                "timeout_config",
                "Maximum polling attempts cannot exceed 200 (too many API calls)",
            ));
        }

        if timeout_config.polling_interval_ms < 50 {
            warn!(
                polling_interval_ms = timeout_config.polling_interval_ms,
                "Timeout configuration has very aggressive polling interval"
            );
            return Err(OmniError::config_invalid(
                "timeout_config",
                "Polling interval cannot be less than 50ms (too aggressive)",
            ));
        }

        if timeout_config.max_total_timeout_secs > 14400 {
            // 4 hours
            warn!(
                max_total_timeout_secs = timeout_config.max_total_timeout_secs,
                "Timeout configuration has excessive total timeout"
            );
            return Err(OmniError::config_invalid(
                "timeout_config",
                "Maximum total timeout cannot exceed 4 hours (14400 seconds)",
            ));
        }

        if timeout_config.polling_backoff_multiplier > 5.0 {
            warn!(
                polling_backoff_multiplier = timeout_config.polling_backoff_multiplier,
                "Timeout configuration has excessive backoff multiplier"
            );
            return Err(OmniError::config_invalid(
                "timeout_config",
                "Polling backoff multiplier cannot exceed 5.0 (too aggressive backoff)",
            ));
        }

        debug!(
            timeout_config = ?timeout_config,
            "Timeout configuration validation passed"
        );

        Ok(())
    }

    pub fn new(base_url: String, api_token: String) -> Result<Self, OmniError> {
        Self::validate_client_config(&base_url, &api_token)?;

        let client = oxy_shared::http::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_token,
            client,
            retry_policy: RetryPolicy::new(RetryConfig::for_api_calls()),
            timeout_config: TimeoutConfig::default(),
        })
    }

    /// Create a new Omni API client with custom retry configuration
    pub fn with_retry_config(
        base_url: String,
        api_token: String,
        retry_config: RetryConfig,
    ) -> Result<Self, OmniError> {
        Self::validate_client_config(&base_url, &api_token)?;

        let client = oxy_shared::http::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_token,
            client,
            retry_policy: RetryPolicy::new(retry_config),
            timeout_config: TimeoutConfig::default(),
        })
    }

    /// Create a new Omni API client with custom timeout configuration
    pub fn with_timeout_config(
        base_url: String,
        api_token: String,
        timeout_config: TimeoutConfig,
    ) -> Result<Self, OmniError> {
        Self::validate_client_config(&base_url, &api_token)?;
        Self::validate_timeout_config(&timeout_config)?;

        let client = oxy_shared::http::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        debug!(
            base_url = %base_url,
            timeout_config = ?timeout_config,
            estimated_max_polling_time_secs = timeout_config.estimate_max_polling_time().as_secs(),
            "Created Omni API client with custom timeout configuration"
        );

        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_token,
            client,
            retry_policy: RetryPolicy::new(RetryConfig::for_api_calls()),
            timeout_config,
        })
    }

    /// Create a new Omni API client optimized for metadata synchronization
    pub fn for_metadata_sync(base_url: String, api_token: String) -> Result<Self, OmniError> {
        Self::validate_client_config(&base_url, &api_token)?;
        Self::with_retry_config(base_url, api_token, RetryConfig::for_metadata_sync())
    }

    fn add_auth_headers(&self, request: RequestBuilder) -> RequestBuilder {
        request.header("Authorization", format!("Bearer {}", self.api_token))
    }

    /// Handle HTTP response and convert to appropriate error if needed
    async fn handle_response<T>(&self, response: reqwest::Response) -> Result<T, OmniError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let status = response.status();

        if status.is_success() {
            response.json::<T>().await.map_err(OmniError::from)
        } else {
            let status_code = status.as_u16();
            let url = response.url().to_string();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            match status_code {
                401 => Err(OmniError::auth_failed(&format!(
                    "Invalid API token or unauthorized access: {}",
                    error_text
                ))),
                403 => Err(OmniError::auth_failed(&format!(
                    "Access forbidden: {}",
                    error_text
                ))),
                404 => Err(OmniError::NotFoundError(format!(
                    "Resource not found at {}: {}",
                    url, error_text
                ))),
                429 => Err(OmniError::RateLimitError(format!(
                    "Rate limit exceeded: {}. Try again later.",
                    error_text
                ))),
                500..=599 => Err(OmniError::ServerError(format!(
                    "Server error ({}): {}",
                    status_code, error_text
                ))),
                _ => Err(OmniError::ApiError {
                    message: error_text,
                    status_code,
                }),
            }
        }
    }

    /// Handle streaming JSON response where each line is a separate JSON object
    /// This is used for query responses that return multiple JSON objects in sequence.
    ///
    /// The Omni query API returns responses in a streaming format with multiple JSON objects:
    /// 1. First object: {"jobs_submitted": {"job_id": "client_result_id"}}
    /// 2. Second object: Complete query result with job details, summary, cache metadata, etc.
    /// 3. Third object: {"remaining_job_ids": [], "timed_out": "false"}
    ///
    /// This method parses each line as a separate JSON object and merges them into a single
    /// QueryResponse structure, ensuring all fields are properly populated.
    async fn handle_streaming_response(
        &self,
        response: reqwest::Response,
    ) -> Result<QueryResponse, OmniError> {
        let status = response.status();

        if !status.is_success() {
            let status_code = status.as_u16();
            let url = response.url().to_string();
            let error_text = response
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());

            return match status_code {
                401 => Err(OmniError::auth_failed(&format!(
                    "Invalid API token or unauthorized access: {}",
                    error_text
                ))),
                403 => Err(OmniError::auth_failed(&format!(
                    "Access forbidden: {}",
                    error_text
                ))),
                404 => Err(OmniError::NotFoundError(format!(
                    "Resource not found at {}: {}",
                    url, error_text
                ))),
                429 => Err(OmniError::RateLimitError(format!(
                    "Rate limit exceeded: {}. Try again later.",
                    error_text
                ))),
                500..=599 => Err(OmniError::ServerError(format!(
                    "Server error ({}): {}",
                    status_code, error_text
                ))),
                _ => Err(OmniError::ApiError {
                    message: error_text,
                    status_code,
                }),
            };
        }

        let response_text = response.text().await.map_err(OmniError::from)?;

        debug!(
            response_length = response_text.len(),
            "Processing streaming response"
        );

        // Parse each line as a separate JSON object and merge them into a single QueryResponse
        let mut final_response = QueryResponse::new();
        let mut line_count = 0;

        for line in response_text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            line_count += 1;
            info!(
                line_number = line_count,
                line_length = line.len(),
                line=%line,
                "Processing response line"
            );

            match serde_json::from_str::<serde_json::Value>(line) {
                Ok(json_value) => {
                    // Try to parse as a partial QueryResponse and merge fields
                    if let Ok(partial_response) =
                        serde_json::from_value::<QueryResponse>(json_value.clone())
                    {
                        self.merge_query_response(&mut final_response, partial_response);
                    } else {
                        // Handle specific object types that might not map directly to QueryResponse
                        if let Some(jobs_submitted) = json_value.get("jobs_submitted")
                            && let Ok(jobs_map) =
                                serde_json::from_value::<std::collections::HashMap<String, String>>(
                                    jobs_submitted.clone(),
                                )
                        {
                            final_response.jobs_submitted = Some(jobs_map);
                        }

                        if let Some(remaining_job_ids) = json_value.get("remaining_job_ids")
                            && let Ok(job_ids) =
                                serde_json::from_value::<Vec<String>>(remaining_job_ids.clone())
                        {
                            final_response.remaining_job_ids = Some(job_ids);
                        }

                        if let Some(timed_out) = json_value.get("timed_out")
                            && let Ok(timed_out_val) =
                                serde_json::from_value::<String>(timed_out.clone())
                        {
                            final_response.timed_out = Some(timed_out_val);
                        }
                    }
                }
                Err(e) => {
                    warn!(
                        line_number = line_count,
                        line_content = %line,
                        error = %e,
                        "Failed to parse JSON line in streaming response"
                    );
                    return Err(OmniError::SerializationError(e));
                }
            }
        }

        info!(
            lines_processed = line_count,
            has_job_id = final_response.job_id.is_some(),
            has_result = final_response.result.is_some(),
            has_jobs_submitted = final_response.jobs_submitted.is_some(),
            has_remaining_jobs = final_response.remaining_job_ids.is_some(),
            "Completed processing streaming response"
        );

        Ok(final_response)
    }

    /// Merge fields from a partial QueryResponse into the final response
    ///
    /// This method implements a merge strategy where fields from the partial response
    /// only overwrite fields in the final response if they are currently None.
    /// This ensures that once a field is set, it won't be overwritten by subsequent
    /// partial responses, maintaining data integrity across the streaming response.
    ///
    /// # Arguments
    /// * `final_response` - The accumulating response that will contain the merged result
    /// * `partial` - A partial response from one line of the streaming JSON
    fn merge_query_response(&self, final_response: &mut QueryResponse, partial: QueryResponse) {
        // Merge scalar fields - only overwrite if final_response field is None
        if final_response.job_id.is_none() && partial.job_id.is_some() {
            final_response.job_id = partial.job_id;
        }
        if final_response.status.is_none() && partial.status.is_some() {
            final_response.status = partial.status;
        }
        if final_response.client_result_id.is_none() && partial.client_result_id.is_some() {
            final_response.client_result_id = partial.client_result_id;
        }
        if final_response.result.is_none() && partial.result.is_some() {
            final_response.result = partial.result;
        }
        if final_response.timed_out.is_none() && partial.timed_out.is_some() {
            final_response.timed_out = partial.timed_out;
        }

        // Merge complex fields - only overwrite if final_response field is None
        if final_response.jobs_submitted.is_none() && partial.jobs_submitted.is_some() {
            final_response.jobs_submitted = partial.jobs_submitted;
        }
        if final_response.summary.is_none() && partial.summary.is_some() {
            final_response.summary = partial.summary;
        }
        if final_response.cache_metadata.is_none() && partial.cache_metadata.is_some() {
            final_response.cache_metadata = partial.cache_metadata;
        }
        if final_response.query.is_none() && partial.query.is_some() {
            final_response.query = partial.query;
        }
        if final_response.stream_stats.is_none() && partial.stream_stats.is_some() {
            final_response.stream_stats = partial.stream_stats;
        }
        if final_response.remaining_job_ids.is_none() && partial.remaining_job_ids.is_some() {
            final_response.remaining_job_ids = partial.remaining_job_ids;
        }
    }

    /// Get the base URL for this client
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Get a reference to the HTTP client for advanced usage
    pub fn http_client(&self) -> &Client {
        &self.client
    }

    /// Create a GET request with authentication headers
    pub fn authenticated_get(&self, url: &str) -> RequestBuilder {
        let request = self.client.get(url);
        self.add_auth_headers(request)
    }

    /// Create a POST request with authentication headers
    pub fn authenticated_post(&self, url: &str) -> RequestBuilder {
        let request = self.client.post(url);
        self.add_auth_headers(request)
    }

    /// Create a PUT request with authentication headers
    pub fn authenticated_put(&self, url: &str) -> RequestBuilder {
        let request = self.client.put(url);
        self.add_auth_headers(request)
    }

    /// Create a DELETE request with authentication headers
    pub fn authenticated_delete(&self, url: &str) -> RequestBuilder {
        let request = self.client.delete(url);
        self.add_auth_headers(request)
    }

    pub async fn execute_request<T>(&self, request: RequestBuilder) -> Result<T, OmniError>
    where
        T: for<'de> Deserialize<'de>,
    {
        let response = request.send().await.map_err(OmniError::HttpError)?;

        self.handle_response(response).await
    }

    /// Execute a request with retry logic
    pub async fn execute_request_with_retry<T, F>(
        &self,
        operation_name: &str,
        request_builder: F,
    ) -> Result<T, OmniError>
    where
        T: for<'de> Deserialize<'de>,
        F: Fn() -> RequestBuilder + Clone,
    {
        let timeout_wrapper = TimeoutWrapper::for_api_calls();
        let operation_start = Instant::now();

        info!(
            operation = operation_name,
            base_url = self.base_url(),
            "Starting request with retry logic"
        );

        let result = timeout_wrapper
            .execute(operation_name, || {
                let request_builder_clone = request_builder.clone();
                let operation_name_clone = operation_name.to_string();
                self.retry_policy.execute(operation_name, move || {
                    let request = request_builder_clone();
                    let operation_name_inner = operation_name_clone.clone();
                    async move {
                        let request_start = Instant::now();
                        let response = request.send().await.map_err(|e| {
                            let request_duration = request_start.elapsed();
                            warn!(
                                error = %e,
                                error_type = std::any::type_name_of_val(&e),
                                operation = %operation_name_inner,
                                request_duration_ms = request_duration.as_millis(),
                                "HTTP request failed, will be retried if temporary"
                            );
                            OmniError::HttpError(e)
                        })?;

                        let request_duration = request_start.elapsed();
                        let status_code = response.status().as_u16();

                        info!(
                            operation = %operation_name_inner,
                            status_code = status_code,
                            request_duration_ms = request_duration.as_millis(),
                            response = ?response,
                            "HTTP request completed"
                        );

                        self.handle_response(response).await
                    }
                })
            })
            .await;

        let operation_duration = operation_start.elapsed();

        match &result {
            Ok(_) => {
                info!(
                    operation = operation_name,
                    operation_duration_ms = operation_duration.as_millis(),
                    "Request with retry completed successfully"
                );
            }
            Err(error) => {
                info!(
                    operation = operation_name,
                    operation_duration_ms = operation_duration.as_millis(),
                    error = %error,
                    error_type = std::any::type_name_of_val(error),
                    is_temporary = error.is_temporary(),
                    retry_delay_secs = error.retry_delay_seconds(),
                    "Request with retry failed"
                );
            }
        }

        result
    }

    pub async fn list_topics(&self, base_model_id: &str) -> Result<ModelsResponse, OmniError> {
        if base_model_id.is_empty() {
            return Err(OmniError::ConfigError(
                "Base model ID cannot be empty".to_string(),
            ));
        }

        let url = format!(
            "{}/api/v1/models?baseModelId={}&modelKind=TOPIC",
            self.base_url,
            urlencoding::encode(base_model_id)
        );

        self.execute_request_with_retry("list_topics", || self.authenticated_get(&url))
            .await
    }

    /// Get detailed information about a specific topic
    pub async fn get_topic(
        &self,
        model_id: &str,
        topic_name: &str,
    ) -> Result<TopicResponse, OmniError> {
        if model_id.is_empty() {
            return Err(OmniError::ConfigError(
                "Model ID cannot be empty".to_string(),
            ));
        }

        if topic_name.is_empty() {
            return Err(OmniError::ConfigError(
                "Topic name cannot be empty".to_string(),
            ));
        }

        let url = format!(
            "{}/api/v1/models/{}/topic/{}",
            self.base_url, model_id, topic_name
        );

        self.execute_request_with_retry("get_topic", || self.authenticated_get(&url))
            .await
    }

    /// Execute a query against the Omni API with automatic timeout handling
    pub async fn execute_query(
        &self,
        query_request: QueryRequest,
    ) -> Result<QueryResponse, OmniError> {
        let query_start = Instant::now();

        self.validate_query_request(&query_request)?;

        let url = format!("{}/api/v1/query/run", self.base_url);

        info!(
            model_id = %query_request.query.model_id,
            fields_count = query_request.query.fields.len(),
            fields = ?query_request.query.fields,
            limit = ?query_request.query.limit,
            sorts_count = query_request.query.sorts.as_ref().map(|s| s.len()).unwrap_or(0),
            user_id = ?query_request.user_id,
            cache = ?query_request.cache,
            result_type = ?query_request.result_type,
            timeout_config = ?self.timeout_config,
            url = %url,
            "Executing query with timeout handling"
        );

        let initial_request_start = Instant::now();
        let query_request_clone = query_request.clone();

        // Use streaming response handler for query execution
        let response: QueryResponse = self
            .retry_policy
            .execute("execute_query", || {
                let url_clone = url.clone();
                let request_clone = query_request_clone.clone();
                async move {
                    let response = self
                        .authenticated_post(&url_clone)
                        .json(&request_clone)
                        .send()
                        .await
                        .map_err(OmniError::HttpError)?;

                    self.handle_streaming_response(response).await
                }
            })
            .await?;

        let initial_request_duration = initial_request_start.elapsed();

        info!(
            model_id = %query_request.query.model_id,
            initial_request_duration_ms = initial_request_duration.as_millis(),
            response_job_id = ?response.job_id,
            response_status = ?response.status,
            has_result = response.result.is_some(),
            has_remaining_jobs = response.remaining_job_ids.as_ref().map(|jobs| !jobs.is_empty()).unwrap_or(false),
            timed_out = ?response.timed_out,
            jobs_submitted = ?response.jobs_submitted,
            "Initial query request completed"
        );

        // Check if query timed out and needs polling
        if response.needs_polling() {
            let remaining_job_ids = response.get_remaining_job_ids();

            warn!(
                model_id = %query_request.query.model_id,
                job_ids = ?remaining_job_ids,
                job_count = remaining_job_ids.len(),
                initial_duration_ms = initial_request_duration.as_millis(),
                timeout_config = ?self.timeout_config,
                estimated_max_polling_time_secs = self.timeout_config.estimate_max_polling_time().as_secs(),
                "Query timed out on initial request, starting polling process"
            );

            let polling_start = Instant::now();
            let poller = QueryPoller::new(self.clone(), self.timeout_config.clone());

            match poller.poll_for_completion(remaining_job_ids.clone()).await {
                Ok(final_response) => {
                    let total_duration = query_start.elapsed();
                    let polling_duration = polling_start.elapsed();

                    debug!(
                        model_id = %query_request.query.model_id,
                        job_ids = ?remaining_job_ids,
                        total_duration_secs = total_duration.as_secs(),
                        total_duration_ms = total_duration.as_millis(),
                        initial_duration_ms = initial_request_duration.as_millis(),
                        polling_duration_secs = polling_duration.as_secs(),
                        polling_duration_ms = polling_duration.as_millis(),
                        final_job_id = ?final_response.job_id,
                        final_status = ?final_response.status,
                        has_result = final_response.result.is_some(),
                        result_size_bytes = final_response.result.as_ref().map(|r| r.len()).unwrap_or(0),
                        "Query completed successfully after polling"
                    );

                    return Ok(final_response);
                }
                Err(polling_error) => {
                    let total_duration = query_start.elapsed();
                    let polling_duration = polling_start.elapsed();

                    warn!(
                        model_id = %query_request.query.model_id,
                        job_ids = ?remaining_job_ids,
                        total_duration_secs = total_duration.as_secs(),
                        initial_duration_ms = initial_request_duration.as_millis(),
                        polling_duration_secs = polling_duration.as_secs(),
                        error = %polling_error,
                        error_type = std::any::type_name_of_val(&polling_error),
                        is_temporary = polling_error.is_temporary(),
                        "Query polling failed"
                    );

                    return Err(polling_error);
                }
            }
        }

        let total_duration = query_start.elapsed();

        info!(
            model_id = %query_request.query.model_id,
            job_id = ?response.job_id,
            status = ?response.status,
            has_result = response.result.is_some(),
            result_size_bytes = response.result.as_ref().map(|r| r.len()).unwrap_or(0),
            total_duration_ms = total_duration.as_millis(),
            cache_hit = response.summary.as_ref()
                .and_then(|s| s.cache_type.as_ref())
                .map(|ct| ct != "MISS")
                .unwrap_or(false),
            response = ?response,
            "Query completed without timeout"
        );

        Ok(response)
    }

    /// Validate a query request before sending to the API
    /// Enhanced to work with new optional fields while maintaining backward compatibility
    fn validate_query_request(&self, query_request: &QueryRequest) -> Result<(), OmniError> {
        let query = &query_request.query;

        // Core required field validations (backward compatible)
        if query.model_id.is_empty() {
            return Err(OmniError::QueryError(
                "Model ID cannot be empty".to_string(),
            ));
        }

        if query.topic.is_empty() {
            return Err(OmniError::QueryError(
                "Topic name cannot be empty".to_string(),
            ));
        }

        if query.fields.is_empty() {
            return Err(OmniError::QueryError(
                "At least one field must be specified".to_string(),
            ));
        }

        // Validate field names are not empty
        for field in &query.fields {
            if field.trim().is_empty() {
                return Err(OmniError::QueryError(
                    "Field names cannot be empty".to_string(),
                ));
            }
        }

        // Enhanced limit validation with more detailed error messages
        if let Some(limit) = query.limit {
            if limit == 0 {
                return Err(OmniError::QueryError(
                    "Limit must be greater than 0".to_string(),
                ));
            }
            if limit > 100000 {
                return Err(OmniError::QueryError(format!(
                    "Limit cannot exceed 100,000 rows (requested: {})",
                    limit
                )));
            }
        }

        // Enhanced sort field validation
        if let Some(sorts) = &query.sorts {
            if sorts.len() > 10 {
                return Err(OmniError::QueryError(format!(
                    "Too many sort fields specified ({}). Maximum allowed is 10",
                    sorts.len()
                )));
            }

            for (index, sort) in sorts.iter().enumerate() {
                if sort.field.trim().is_empty() {
                    return Err(OmniError::QueryError(format!(
                        "Sort field at index {} cannot be empty",
                        index
                    )));
                }

                // Validate sort field name format (basic validation)
                if sort.field.len() > 255 {
                    return Err(OmniError::QueryError(format!(
                        "Sort field name '{}' is too long (max 255 characters)",
                        sort.field
                    )));
                }
            }
        }

        // Validate optional fields when present (enhanced model support)
        if let Some(user_id) = &query_request.user_id {
            if user_id.trim().is_empty() {
                return Err(OmniError::QueryError(
                    "User ID cannot be empty when specified".to_string(),
                ));
            }
            if user_id.len() > 255 {
                return Err(OmniError::QueryError(
                    "User ID cannot exceed 255 characters".to_string(),
                ));
            }
        }

        if let Some(cache) = &query_request.cache {
            if cache.trim().is_empty() {
                return Err(OmniError::QueryError(
                    "Cache setting cannot be empty when specified".to_string(),
                ));
            }
            // Validate cache values against known options
            let valid_cache_options = ["true", "false", "refresh", "bypass"];
            if !valid_cache_options.contains(&cache.to_lowercase().as_str()) {
                return Err(OmniError::QueryError(format!(
                    "Invalid cache option '{}'. Valid options: {}",
                    cache,
                    valid_cache_options.join(", ")
                )));
            }
        }

        if let Some(result_type) = &query_request.result_type {
            if result_type.trim().is_empty() {
                return Err(OmniError::QueryError(
                    "Result type cannot be empty when specified".to_string(),
                ));
            }
            // Validate result type against known formats
            let valid_result_types = ["json", "csv", "arrow", "parquet"];
            if !valid_result_types.contains(&result_type.to_lowercase().as_str()) {
                return Err(OmniError::QueryError(format!(
                    "Invalid result type '{}'. Valid types: {}",
                    result_type,
                    valid_result_types.join(", ")
                )));
            }
        }

        // Validate field name format (enhanced validation)
        for (index, field) in query.fields.iter().enumerate() {
            if field.len() > 500 {
                return Err(OmniError::QueryError(format!(
                    "Field name at index {} is too long (max 500 characters): '{}'",
                    index, field
                )));
            }

            // Basic format validation - field names should not contain certain characters
            if field.contains('\n') || field.contains('\r') || field.contains('\t') {
                return Err(OmniError::QueryError(format!(
                    "Field name at index {} contains invalid characters: '{}'",
                    index, field
                )));
            }
        }

        // Validate model_id format (enhanced validation)
        if query.model_id.len() > 255 {
            return Err(OmniError::QueryError(
                "Model ID cannot exceed 255 characters".to_string(),
            ));
        }

        // Validate topic name format (enhanced validation)
        if query.topic.len() > 255 {
            return Err(OmniError::QueryError(
                "Topic name cannot exceed 255 characters".to_string(),
            ));
        }

        Ok(())
    }
}

/// QueryPoller handles polling for query completion when queries timeout
/// and return remaining_job_ids that need to be polled via /api/v1/query/wait
///
/// # Logging
///
/// The QueryPoller provides comprehensive logging for timeout operations:
///
/// - **Debug logs**: Detailed information about polling attempts, intervals, job IDs, and timing
/// - **Warning logs**: Timeout scenarios, retry attempts, and error conditions
/// - **Structured fields**: All logs include structured fields for job IDs, timing, and metadata
///
/// Example log output:
/// ```text
/// DEBUG omni::client: Starting query polling with timeout configuration
///   job_ids=["abc-123", "def-456"] max_attempts=20 initial_interval_ms=2000
///
/// DEBUG omni::client: Polling attempt for query completion
///   attempt=1 elapsed_secs=0 current_interval_ms=2000 progress_pct=5
///
/// WARN omni::client: Query polling exceeded maximum total timeout
///   elapsed_secs=300 max_timeout_secs=300 attempts_made=15
/// ```
#[derive(Debug, Clone)]
pub struct QueryPoller {
    client: OmniApiClient,
    config: TimeoutConfig,
    retry_policy: RetryPolicy,
}

impl QueryPoller {
    /// Create a new QueryPoller with the given client and timeout configuration
    pub fn new(client: OmniApiClient, config: TimeoutConfig) -> Self {
        debug!(
            base_url = client.base_url(),
            timeout_config = ?config,
            estimated_max_time_secs = config.estimate_max_polling_time().as_secs(),
            "Creating new QueryPoller with default retry policy"
        );

        Self {
            client,
            config,
            retry_policy: RetryPolicy::new(RetryConfig::for_api_calls()),
        }
    }

    /// Create a QueryPoller with custom retry policy
    pub fn with_retry_policy(
        client: OmniApiClient,
        config: TimeoutConfig,
        retry_policy: RetryPolicy,
    ) -> Self {
        debug!(
            base_url = client.base_url(),
            timeout_config = ?config,
            estimated_max_time_secs = config.estimate_max_polling_time().as_secs(),
            "Creating new QueryPoller with custom retry policy"
        );

        Self {
            client,
            config,
            retry_policy,
        }
    }

    /// Poll for query completion using the /api/v1/query/wait endpoint
    ///
    /// This method implements exponential backoff logic and continues polling
    /// until the query completes, times out, or exceeds maximum attempts.
    pub async fn poll_for_completion(
        &self,
        job_ids: Vec<String>,
    ) -> Result<QueryResponse, OmniError> {
        if job_ids.is_empty() {
            return Err(OmniError::QueryPollingError(
                "No job IDs provided for polling".to_string(),
            ));
        }

        let start_time = Instant::now();
        let mut attempt = 1;
        let mut current_interval = self.config.get_initial_polling_interval();
        let estimated_max_time = self.config.estimate_max_polling_time();

        debug!(
            job_ids = ?job_ids,
            job_count = job_ids.len(),
            max_attempts = self.config.max_polling_attempts,
            initial_interval_ms = self.config.polling_interval_ms,
            total_timeout_secs = self.config.max_total_timeout_secs,
            backoff_multiplier = self.config.polling_backoff_multiplier,
            max_interval_ms = self.config.max_polling_interval_ms,
            estimated_max_time_secs = estimated_max_time.as_secs(),
            "Starting query polling with timeout configuration"
        );

        loop {
            let elapsed = start_time.elapsed();
            let elapsed_secs = elapsed.as_secs();
            let elapsed_ms = elapsed.as_millis();

            if elapsed > self.config.get_total_timeout() {
                warn!(
                    job_ids = ?job_ids,
                    elapsed_secs = elapsed_secs,
                    max_timeout_secs = self.config.max_total_timeout_secs,
                    attempts_made = attempt - 1,
                    "Query polling exceeded maximum total timeout"
                );
                return Err(OmniError::QueryTimeoutError(format!(
                    "Query polling exceeded maximum total timeout of {} seconds (elapsed: {} seconds, attempts: {})",
                    self.config.max_total_timeout_secs,
                    elapsed_secs,
                    attempt - 1
                )));
            }

            if attempt > self.config.max_polling_attempts {
                warn!(
                    job_ids = ?job_ids,
                    elapsed_secs = elapsed_secs,
                    max_attempts = self.config.max_polling_attempts,
                    "Query polling exceeded maximum attempts"
                );
                return Err(OmniError::QueryTimeoutError(format!(
                    "Query polling exceeded maximum attempts of {} (elapsed: {} seconds)",
                    self.config.max_polling_attempts, elapsed_secs
                )));
            }

            debug!(
                attempt = attempt,
                max_attempts = self.config.max_polling_attempts,
                job_ids = ?job_ids,
                job_count = job_ids.len(),
                elapsed_secs = elapsed_secs,
                elapsed_ms = elapsed_ms,
                current_interval_ms = current_interval.as_millis(),
                remaining_timeout_secs = self.config.max_total_timeout_secs.saturating_sub(elapsed_secs),
                progress_pct = (attempt as f64 / self.config.max_polling_attempts as f64 * 100.0) as u32,
                "Polling attempt for query completion"
            );

            // Make polling request
            let poll_start = Instant::now();
            match self.poll_once(&job_ids).await {
                Ok(response) => {
                    let poll_duration = poll_start.elapsed();

                    // Check if query is complete
                    if response.is_complete() {
                        debug!(
                            attempt = attempt,
                            total_duration_secs = elapsed_secs,
                            total_duration_ms = elapsed_ms,
                            poll_duration_ms = poll_duration.as_millis(),
                            job_ids = ?job_ids,
                            job_count = job_ids.len(),
                            final_status = ?response.status,
                            has_result = response.result.is_some(),
                            result_size_bytes = response.result.as_ref().map(|r| r.len()).unwrap_or(0),
                            "Query completed successfully after polling"
                        );
                        return Ok(response);
                    }

                    // Query still processing, continue polling
                    debug!(
                        attempt = attempt,
                        next_interval_ms = current_interval.as_millis(),
                        elapsed_secs = elapsed_secs,
                        poll_duration_ms = poll_duration.as_millis(),
                        job_ids = ?job_ids,
                        timed_out_status = ?response.timed_out,
                        remaining_attempts = self.config.max_polling_attempts.saturating_sub(attempt),
                        "Query still processing, scheduling next poll"
                    );
                }
                Err(error) => {
                    let poll_duration = poll_start.elapsed();

                    // Use retry policy for transient errors
                    if error.is_temporary() {
                        warn!(
                            attempt = attempt,
                            error = %error,
                            error_type = std::any::type_name_of_val(&error),
                            job_ids = ?job_ids,
                            poll_duration_ms = poll_duration.as_millis(),
                            elapsed_secs = elapsed_secs,
                            retry_delay_secs = error.retry_delay_seconds(),
                            "Polling request failed with temporary error, will retry on next attempt"
                        );
                        // Continue with the polling loop - the error will be retried on the next attempt
                    } else {
                        warn!(
                            attempt = attempt,
                            error = %error,
                            error_type = std::any::type_name_of_val(&error),
                            job_ids = ?job_ids,
                            poll_duration_ms = poll_duration.as_millis(),
                            elapsed_secs = elapsed_secs,
                            "Polling failed with non-temporary error, aborting"
                        );
                        return Err(OmniError::QueryPollingError(format!(
                            "Polling failed with non-temporary error after {} attempts (elapsed: {} seconds): {}",
                            attempt, elapsed_secs, error
                        )));
                    }
                }
            }

            // Log before waiting
            debug!(
                attempt = attempt,
                wait_duration_ms = current_interval.as_millis(),
                next_attempt = attempt + 1,
                elapsed_secs = elapsed_secs,
                "Waiting before next polling attempt"
            );

            // Wait before next attempt
            tokio::time::sleep(current_interval).await;

            // Update interval with exponential backoff
            let previous_interval = current_interval;
            current_interval = self.config.calculate_next_interval(current_interval);

            debug!(
                attempt = attempt,
                previous_interval_ms = previous_interval.as_millis(),
                new_interval_ms = current_interval.as_millis(),
                backoff_multiplier = self.config.polling_backoff_multiplier,
                max_interval_ms = self.config.max_polling_interval_ms,
                "Updated polling interval with exponential backoff"
            );

            attempt += 1;
        }
    }

    /// Make a single polling request to the /api/v1/query/wait endpoint
    async fn poll_once(&self, job_ids: &[String]) -> Result<QueryResponse, OmniError> {
        let job_ids_param = job_ids.join(",");
        let url = format!(
            "{}/api/v1/query/wait?jobIds={}",
            self.client.base_url(),
            urlencoding::encode(&job_ids_param)
        );

        debug!(
            url = %url,
            job_ids = ?job_ids,
            job_count = job_ids.len(),
            job_ids_param_length = job_ids_param.len(),
            base_url = self.client.base_url(),
            "Making polling request to wait endpoint"
        );

        let request_start = Instant::now();

        // Use the retry policy for this individual request
        let result = self
            .retry_policy
            .execute("poll_query_wait", || {
                let url_clone = url.clone();
                let job_ids_clone = job_ids.to_vec();
                let request = self.client.authenticated_get(&url);
                async move {
                    let http_start = Instant::now();
                    let response = request.send().await.map_err(|e| {
                        warn!(
                            error = %e,
                            error_type = std::any::type_name_of_val(&e),
                            url = %url_clone,
                            job_ids = ?job_ids_clone,
                            http_duration_ms = http_start.elapsed().as_millis(),
                            "HTTP request to wait endpoint failed"
                        );
                        OmniError::HttpError(e)
                    })?;

                    let status_code = response.status().as_u16();
                    let response_headers = response.headers().clone();

                    debug!(
                        url = %url_clone,
                        job_ids = ?job_ids_clone,
                        status_code = status_code,
                        http_duration_ms = http_start.elapsed().as_millis(),
                        content_length = response_headers.get("content-length")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("unknown"),
                        "Received response from wait endpoint"
                    );

                    self.client.handle_streaming_response(response).await
                }
            })
            .await;

        let request_duration = request_start.elapsed();

        match &result {
            Ok(response) => {
                debug!(
                    job_ids = ?job_ids,
                    request_duration_ms = request_duration.as_millis(),
                    response_job_id = ?response.job_id,
                    response_status = ?response.status,
                    response_timed_out = ?response.timed_out,
                    has_result = response.result.is_some(),
                    has_remaining_jobs = response.remaining_job_ids.as_ref().map(|jobs| !jobs.is_empty()).unwrap_or(false),
                    result_size_bytes = response.result.as_ref().map(|r| r.len()).unwrap_or(0),
                    "Successfully received polling response"
                );
            }
            Err(error) => {
                warn!(
                    job_ids = ?job_ids,
                    request_duration_ms = request_duration.as_millis(),
                    error = %error,
                    error_type = std::any::type_name_of_val(error),
                    is_temporary = error.is_temporary(),
                    retry_delay_secs = error.retry_delay_seconds(),
                    "Polling request failed"
                );
            }
        }

        result
    }

    /// Get the timeout configuration for this poller
    pub fn timeout_config(&self) -> &TimeoutConfig {
        &self.config
    }

    /// Get the retry policy for this poller
    pub fn retry_policy(&self) -> &RetryPolicy {
        &self.retry_policy
    }

    /// Estimate the maximum time this poller might take
    pub fn estimate_max_polling_time(&self) -> Duration {
        self.config.estimate_max_polling_time()
    }
}

impl OmniApiClient {
    /// Get the timeout configuration for this client
    pub fn timeout_config(&self) -> &TimeoutConfig {
        &self.timeout_config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_merge_query_response() {
        let client = OmniApiClient::new(
            "https://test.example.com".to_string(),
            "test-token".to_string(),
        )
        .unwrap();

        let mut final_response = QueryResponse::new();

        // First partial response with jobs_submitted
        let partial1 = QueryResponse {
            jobs_submitted: Some({
                let mut map = HashMap::new();
                map.insert("job1".to_string(), "result1".to_string());
                map
            }),
            ..QueryResponse::new()
        };

        // Second partial response with job details
        let partial2 = QueryResponse {
            job_id: Some("job1".to_string()),
            status: Some("COMPLETE".to_string()),
            result: Some("test-result".to_string()),
            ..QueryResponse::new()
        };

        // Third partial response with timeout info
        let partial3 = QueryResponse {
            remaining_job_ids: Some(vec![]),
            timed_out: Some("false".to_string()),
            ..QueryResponse::new()
        };

        client.merge_query_response(&mut final_response, partial1);
        client.merge_query_response(&mut final_response, partial2);
        client.merge_query_response(&mut final_response, partial3);

        // Verify all fields were merged correctly
        assert!(final_response.jobs_submitted.is_some());
        assert_eq!(final_response.job_id.as_ref().unwrap(), "job1");
        assert_eq!(final_response.status.as_ref().unwrap(), "COMPLETE");
        assert_eq!(final_response.result.as_ref().unwrap(), "test-result");
        assert!(final_response.remaining_job_ids.is_some());
        assert_eq!(final_response.timed_out.as_ref().unwrap(), "false");
    }
}
