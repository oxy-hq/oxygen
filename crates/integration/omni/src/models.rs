use garde::Validate;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Response structure for listing models from Omni API
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ModelsResponse {
    pub page_info: PageInfo,
    pub records: Vec<ModelRecord>,
}

/// Pagination information for API responses
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PageInfo {
    pub has_next_page: bool,
    pub has_previous_page: bool,
    pub start_cursor: Option<String>,
    pub end_cursor: Option<String>,
    pub total_count: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ModelRecord {
    pub id: String,
    pub name: String,
    pub label: Option<String>,
    pub description: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

/// Response structure for getting topic details from Omni API
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TopicResponse {
    pub success: bool,
    pub topic: TopicData,
}

/// Topic data from the Omni API
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TopicData {
    pub name: String,
    pub label: Option<String>,
    pub base_view_name: String,
    pub views: Vec<ViewData>,
}

/// View data from the Omni API
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ViewData {
    pub name: String,
    pub dimensions: Vec<DimensionData>,
    pub measures: Vec<MeasureData>,
    pub filter_only_fields: Vec<String>,
}

/// Dimension data from the Omni API
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct DimensionData {
    pub field_name: String,
    pub view_name: String,
    pub data_type: String,
    pub fully_qualified_name: String,
    pub description: Option<String>,
    pub ai_context: Option<String>,
    pub label: Option<String>,
}

/// Measure data from the Omni API
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct MeasureData {
    pub field_name: String,
    pub view_name: String,
    pub data_type: String,
    pub fully_qualified_name: String,
    pub description: Option<String>,
    pub ai_context: Option<String>,
    pub label: Option<String>,
}

/// Query request structure for executing queries against Omni API
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct QueryRequest {
    pub query: QueryStructure,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_type: Option<String>,
}

/// Query structure defining the actual query to execute
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct QueryStructure {
    #[serde(rename = "join_paths_from_topic_name")]
    pub topic: String,
    pub fields: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sorts: Option<Vec<SortField>>,
    #[serde(rename = "modelId")]
    pub model_id: String,
}

/// Sort field specification for queries
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SortField {
    pub field: String,
    pub sort_descending: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "lowercase")]
pub enum SortDirection {
    Asc,
    Desc,
}

/// Configuration for query timeout and polling behavior
#[derive(Debug, Clone, Validate)]
pub struct TimeoutConfig {
    /// Maximum number of polling attempts for timed-out queries
    #[garde(range(min = 1, max = 100))]
    pub max_polling_attempts: u32,

    /// Interval between polling attempts (in milliseconds)
    #[garde(range(min = 100, max = 300000))] // 100ms to 5 minutes
    pub polling_interval_ms: u64,

    /// Maximum total time to wait for query completion (in seconds)
    #[garde(range(min = 10, max = 7200))] // 10 seconds to 2 hours
    pub max_total_timeout_secs: u64,

    /// Exponential backoff multiplier for polling intervals
    #[garde(range(min = 1.0, max = 3.0))]
    pub polling_backoff_multiplier: f64,

    /// Maximum polling interval (in milliseconds)
    #[garde(range(min = 1000, max = 600000))] // 1 second to 10 minutes
    pub max_polling_interval_ms: u64,
}

/// Response structure for query execution - API-compliant model
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct QueryResponse {
    // Initial response from /api/v1/query/run
    pub jobs_submitted: Option<std::collections::HashMap<String, String>>,

    // Job details (present when query completes or during polling)
    pub job_id: Option<String>,
    pub status: Option<String>,
    pub client_result_id: Option<String>,
    pub error_type: Option<String>,
    pub error_message: Option<String>,

    // Query execution details
    pub summary: Option<QuerySummary>,
    pub cache_metadata: Option<CacheMetadata>,
    pub query: Option<QueryDetails>,

    // Result data (base64 encoded Apache Arrow or specified format)
    pub result: Option<String>,
    pub file_path: Option<String>,
    pub stream_stats: Option<std::collections::HashMap<String, serde_json::Value>>,

    // Timeout handling
    pub remaining_job_ids: Option<Vec<String>>,
    pub timed_out: Option<String>, // "true" or "false" as string
}

/// Query summary containing execution details and SQL information
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct QuerySummary {
    pub cache_type: Option<String>,
    pub display_sql: Option<String>,
    pub omni_sql: Option<String>,
    pub stage_summaries: Option<Vec<serde_json::Value>>,
    pub omni_sql_parse_failed: Option<bool>,
    pub stats: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub plan_stats: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub fields: Option<std::collections::HashMap<String, FieldInfo>>,
    pub missing_fields: Option<Vec<String>>,
    pub invalid_calculations: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// Field information in query results
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct FieldInfo {
    pub field_name: String,
    pub view_name: Option<String>,
    pub data_type: String,
    pub is_dimension: Option<bool>,
    pub fully_qualified_name: String,
    pub aggregate_type: Option<String>,
    pub filters: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub ignored: Option<bool>,
    pub label: Option<String>,
    pub format: Option<String>,
    pub display_sql: Option<String>,
}

/// Cache metadata for query results
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct CacheMetadata {
    pub plan_key: Option<String>,
    pub field_list: Option<Vec<String>>,
    pub num_rows: Option<u32>,
    pub created_at: Option<u64>,
    pub data_fresh_at: Option<u64>,
    pub bytes: Option<u64>,
    pub ttl: Option<u32>,
    pub model_id: Option<String>,
}

/// Query details containing model job information
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct QueryDetails {
    pub model_job: Option<ModelJob>,
}

/// Model job specification for query execution
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ModelJob {
    pub model_id: String,
    pub table: String,
    pub fields: Vec<String>,
    pub calculations: Option<Vec<serde_json::Value>>,
    pub filters: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub sorts: Option<Vec<SortSpec>>,
    pub limit: Option<u32>,
    pub pivots: Option<Vec<serde_json::Value>>,
    pub fill_fields: Option<Vec<String>>,
    pub column_totals: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub row_totals: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub column_limit: Option<u32>,
    pub default_group_by: Option<bool>,
    pub join_via_map: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub join_paths_from_topic_name: Option<String>,
    pub client_result_id: Option<String>,
    pub version: Option<u32>,
    pub period_over_period_computations: Option<Vec<serde_json::Value>>,
    pub query_references: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub metadata: Option<std::collections::HashMap<String, serde_json::Value>>,
    pub custom_summary_types: Option<std::collections::HashMap<String, serde_json::Value>>,
}

/// Sort specification for API-compliant sorting
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SortSpec {
    pub column_name: String,
    pub sort_descending: bool,
    pub is_column_sort: Option<bool>,
    pub null_sort: Option<String>,
}

/// Legacy query result data for backward compatibility
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct QueryData {
    pub rows: Vec<serde_json::Value>,
    pub columns: Vec<ColumnInfo>,
    pub row_count: u32,
}

/// Legacy column information for backward compatibility
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub label: Option<String>,
}

/// Legacy query execution metadata for backward compatibility
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct QueryMetadata {
    pub execution_time_ms: Option<u64>,
    pub cache_hit: Option<bool>,
    pub query_id: Option<String>,
}

// Builder implementations for query structures
impl QueryRequest {
    pub fn builder() -> QueryRequestBuilder {
        QueryRequestBuilder::new()
    }
}

impl QueryStructure {
    pub fn builder() -> QueryStructureBuilder {
        QueryStructureBuilder::new()
    }
}

#[derive(Debug, Default)]
pub struct QueryRequestBuilder {
    query: Option<QueryStructure>,
    user_id: Option<String>,
    cache: Option<String>,
    result_type: Option<String>,
}

impl QueryRequestBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn query(mut self, query: QueryStructure) -> Self {
        self.query = Some(query);
        self
    }

    pub fn user_id<S: Into<String>>(mut self, user_id: S) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    pub fn cache<S: Into<String>>(mut self, cache: S) -> Self {
        self.cache = Some(cache.into());
        self
    }

    pub fn result_type<S: Into<String>>(mut self, result_type: S) -> Self {
        self.result_type = Some(result_type.into());
        self
    }

    pub fn build(self) -> Result<QueryRequest, String> {
        let query = self.query.ok_or("Query structure is required")?;

        Ok(QueryRequest {
            query,
            user_id: self.user_id,
            cache: self.cache,
            result_type: self.result_type,
        })
    }
}

#[derive(Debug, Default)]
pub struct QueryStructureBuilder {
    topic: Option<String>,
    fields: Vec<String>,
    limit: Option<u32>,
    sorts: Option<Vec<SortField>>,
    model_id: Option<String>,
    version: u32,
}

impl QueryStructureBuilder {
    pub fn new() -> Self {
        Self {
            version: 1, // Default version
            ..Default::default()
        }
    }

    pub fn topic<S: Into<String>>(mut self, topic: S) -> Self {
        self.topic = Some(topic.into());
        self
    }

    pub fn field<S: Into<String>>(mut self, field: S) -> Self {
        self.fields.push(field.into());
        self
    }

    pub fn fields<I, S>(mut self, fields: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.fields.extend(fields.into_iter().map(|f| f.into()));
        self
    }

    pub fn limit(mut self, limit: u32) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn sort(mut self, field: String, direction: SortDirection) -> Self {
        if self.sorts.is_none() {
            self.sorts = Some(Vec::new());
        }
        self.sorts.as_mut().unwrap().push(SortField {
            field,
            sort_descending: matches!(direction, SortDirection::Desc),
        });
        self
    }

    pub fn sorts(mut self, sorts: Vec<SortField>) -> Self {
        self.sorts = Some(sorts);
        self
    }

    pub fn model_id<S: Into<String>>(mut self, model_id: S) -> Self {
        self.model_id = Some(model_id.into());
        self
    }

    pub fn version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    pub fn build(self) -> Result<QueryStructure, String> {
        let topic = self.topic.ok_or("Topic name is required")?;
        let model_id = self.model_id.ok_or("Model ID is required")?;

        if self.fields.is_empty() {
            return Err("At least one field is required".to_string());
        }

        Ok(QueryStructure {
            topic,
            fields: self.fields,
            // limit: self.limit.or(Some(1000)),
            limit: self.limit,
            sorts: self.sorts,
            model_id,
        })
    }
}

impl Default for TimeoutConfig {
    fn default() -> Self {
        Self {
            max_polling_attempts: 20,
            polling_interval_ms: 2000,   // 2 seconds
            max_total_timeout_secs: 300, // 5 minutes
            polling_backoff_multiplier: 1.5,
            max_polling_interval_ms: 30000, // 30 seconds
        }
    }
}

impl TimeoutConfig {
    /// Creates a new TimeoutConfig with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a timeout configuration optimized for quick queries
    /// - Lower timeout limits
    /// - More frequent polling
    /// - Faster backoff
    pub fn for_quick_queries() -> Self {
        Self {
            max_polling_attempts: 10,
            polling_interval_ms: 1000,  // 1 second
            max_total_timeout_secs: 60, // 1 minute
            polling_backoff_multiplier: 1.2,
            max_polling_interval_ms: 10000, // 10 seconds
        }
    }

    /// Creates a timeout configuration optimized for long-running queries
    /// - Higher timeout limits
    /// - Less frequent polling to reduce API load
    /// - Slower backoff for efficiency
    pub fn for_long_running_queries() -> Self {
        Self {
            max_polling_attempts: 50,
            polling_interval_ms: 5000,    // 5 seconds
            max_total_timeout_secs: 1800, // 30 minutes
            polling_backoff_multiplier: 1.3,
            max_polling_interval_ms: 60000, // 1 minute
        }
    }

    /// Creates a timeout configuration for development/testing
    /// - Very short timeouts for quick feedback
    /// - Frequent polling for responsiveness
    pub fn for_development() -> Self {
        Self {
            max_polling_attempts: 5,
            polling_interval_ms: 500,   // 0.5 seconds
            max_total_timeout_secs: 30, // 30 seconds
            polling_backoff_multiplier: 1.1,
            max_polling_interval_ms: 5000, // 5 seconds
        }
    }

    /// Validates the timeout configuration and returns detailed validation errors
    pub fn validate_config(&self) -> Result<(), garde::Report> {
        self.validate()
    }

    /// Validates that the configuration values are reasonable and consistent
    pub fn validate_consistency(&self) -> Result<(), String> {
        if self.max_polling_interval_ms < self.polling_interval_ms {
            return Err(
                "Maximum polling interval must be greater than or equal to initial polling interval"
                    .to_string(),
            );
        }

        if self.polling_backoff_multiplier < 1.0 {
            return Err("Polling backoff multiplier must be at least 1.0".to_string());
        }

        // Check that total timeout allows for at least one polling attempt
        let min_time_needed =
            (self.polling_interval_ms as f64 / 1000.0) * self.max_polling_attempts as f64;
        if (self.max_total_timeout_secs as f64) < min_time_needed {
            return Err(format!(
                "Total timeout ({} seconds) is too short for {} polling attempts with {}ms intervals. Need at least {:.1} seconds",
                self.max_total_timeout_secs,
                self.max_polling_attempts,
                self.polling_interval_ms,
                min_time_needed
            ));
        }

        Ok(())
    }

    pub fn get_initial_polling_interval(&self) -> Duration {
        Duration::from_millis(self.polling_interval_ms)
    }

    pub fn get_max_polling_interval(&self) -> Duration {
        Duration::from_millis(self.max_polling_interval_ms)
    }

    pub fn get_total_timeout(&self) -> Duration {
        Duration::from_secs(self.max_total_timeout_secs)
    }

    /// Calculates the next polling interval with exponential backoff
    pub fn calculate_next_interval(&self, current_interval: Duration) -> Duration {
        let next_interval_ms =
            (current_interval.as_millis() as f64 * self.polling_backoff_multiplier) as u64;
        Duration::from_millis(next_interval_ms.min(self.max_polling_interval_ms))
    }

    /// Estimates the total time needed for all polling attempts (worst case)
    pub fn estimate_max_polling_time(&self) -> Duration {
        let mut total_time = 0u64;
        let mut current_interval = self.polling_interval_ms;

        for _ in 0..self.max_polling_attempts {
            total_time += current_interval;
            current_interval = (current_interval as f64 * self.polling_backoff_multiplier) as u64;
            current_interval = current_interval.min(self.max_polling_interval_ms);
        }

        Duration::from_millis(total_time)
    }

    /// Creates a custom timeout configuration with validation
    pub fn custom(
        max_polling_attempts: u32,
        polling_interval_ms: u64,
        max_total_timeout_secs: u64,
        polling_backoff_multiplier: f64,
        max_polling_interval_ms: u64,
    ) -> Result<Self, String> {
        let config = Self {
            max_polling_attempts,
            polling_interval_ms,
            max_total_timeout_secs,
            polling_backoff_multiplier,
            max_polling_interval_ms,
        };

        // Validate using garde
        config
            .validate_config()
            .map_err(|e| format!("Validation failed: {}", e))?;

        config.validate_consistency()?;

        Ok(config)
    }
}

// Implementations for new API-compliant query response models

impl QueryResponse {
    /// Creates a new QueryResponse with default values
    pub fn new() -> Self {
        Self {
            jobs_submitted: None,
            job_id: None,
            status: None,
            client_result_id: None,
            summary: None,
            cache_metadata: None,
            query: None,
            result: None,
            file_path: None,
            stream_stats: None,
            remaining_job_ids: None,
            timed_out: None,
            error_type: None,
            error_message: None,
        }
    }

    /// Checks if the query has timed out and needs polling
    pub fn has_timed_out(&self) -> bool {
        self.timed_out.as_ref().is_some_and(|t| t == "true")
    }

    /// Checks if the query is complete (not timed out)
    pub fn is_complete(&self) -> bool {
        self.timed_out.as_ref().is_none_or(|t| t == "false")
    }

    /// Gets the remaining job IDs for polling
    pub fn get_remaining_job_ids(&self) -> Vec<String> {
        self.remaining_job_ids.clone().unwrap_or_default()
    }

    /// Checks if this response has job IDs that need polling
    pub fn needs_polling(&self) -> bool {
        self.remaining_job_ids
            .as_ref()
            .is_some_and(|ids| !ids.is_empty())
    }
}

impl Default for QueryResponse {
    fn default() -> Self {
        Self::new()
    }
}

impl QuerySummary {
    /// Creates a new QuerySummary with default values
    pub fn new() -> Self {
        Self {
            cache_type: None,
            display_sql: None,
            omni_sql: None,
            stage_summaries: None,
            omni_sql_parse_failed: None,
            stats: None,
            plan_stats: None,
            fields: None,
            missing_fields: None,
            invalid_calculations: None,
        }
    }

    /// Gets the display SQL if available
    pub fn get_display_sql(&self) -> Option<&str> {
        self.display_sql.as_deref()
    }

    /// Gets the Omni SQL if available
    pub fn get_omni_sql(&self) -> Option<&str> {
        self.omni_sql.as_deref()
    }

    /// Checks if the query had parsing failures
    pub fn has_parse_failures(&self) -> bool {
        self.omni_sql_parse_failed.unwrap_or(false)
    }

    pub fn get_field_info(&self, field_name: &str) -> Option<&FieldInfo> {
        self.fields.as_ref()?.get(field_name)
    }
}

impl Default for QuerySummary {
    fn default() -> Self {
        Self::new()
    }
}

impl FieldInfo {
    /// Creates a new FieldInfo with required fields
    pub fn new(field_name: String, data_type: String, fully_qualified_name: String) -> Self {
        Self {
            field_name,
            view_name: None,
            data_type,
            is_dimension: None,
            fully_qualified_name,
            aggregate_type: None,
            filters: None,
            ignored: None,
            label: None,
            format: None,
            display_sql: None,
        }
    }
}

impl CacheMetadata {
    /// Creates a new CacheMetadata with default values
    pub fn new() -> Self {
        Self {
            plan_key: None,
            field_list: None,
            num_rows: None,
            created_at: None,
            data_fresh_at: None,
            bytes: None,
            ttl: None,
            model_id: None,
        }
    }
}

impl Default for CacheMetadata {
    fn default() -> Self {
        Self::new()
    }
}

impl QueryDetails {
    /// Creates a new QueryDetails with default values
    pub fn new() -> Self {
        Self { model_job: None }
    }
}

impl Default for QueryDetails {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelJob {
    /// Creates a new ModelJob with required fields
    pub fn new(model_id: String, table: String, fields: Vec<String>) -> Self {
        Self {
            model_id,
            table,
            fields,
            calculations: None,
            filters: None,
            sorts: None,
            limit: None,
            pivots: None,
            fill_fields: None,
            column_totals: None,
            row_totals: None,
            column_limit: None,
            default_group_by: None,
            join_via_map: None,
            join_paths_from_topic_name: None,
            client_result_id: None,
            version: None,
            period_over_period_computations: None,
            query_references: None,
            metadata: None,
            custom_summary_types: None,
        }
    }
}

impl SortSpec {
    /// Creates a new SortSpec
    pub fn new(column_name: String, sort_descending: bool) -> Self {
        Self {
            column_name,
            sort_descending,
            is_column_sort: None,
            null_sort: None,
        }
    }
}
#[cfg(test)]
mod model_record_tests {
    use super::*;

    #[test]
    fn test_model_record_serde() {
        let record = ModelRecord {
            id: "model_123".to_string(),
            name: "test_model".to_string(),
            label: Some("Test Model".to_string()),
            description: Some("A test model".to_string()),
            created_at: Some("2024-01-01T00:00:00Z".to_string()),
            updated_at: Some("2024-01-02T00:00:00Z".to_string()),
        };

        let json = serde_json::to_string(&record).unwrap();
        let deserialized: ModelRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.id, record.id);
        assert_eq!(deserialized.name, record.name);
    }

    #[test]
    fn test_model_record_without_optional_fields() {
        let record = ModelRecord {
            id: "123".to_string(),
            name: "model".to_string(),
            label: None,
            description: None,
            created_at: None,
            updated_at: None,
        };

        let json = serde_json::to_string(&record).unwrap();
        let deserialized: ModelRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, record.id);
        assert!(deserialized.label.is_none());
    }
}

#[cfg(test)]
mod page_info_tests {
    use super::*;

    #[test]
    fn test_page_info_serialization() {
        let page_info = PageInfo {
            has_next_page: true,
            has_previous_page: false,
            start_cursor: Some("cursor_start".to_string()),
            end_cursor: Some("cursor_end".to_string()),
            total_count: Some(100),
        };

        let json = serde_json::to_string(&page_info).unwrap();
        let deserialized: PageInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.has_next_page, true);
        assert_eq!(deserialized.total_count, Some(100));
    }
}

#[cfg(test)]
mod constructor_tests {
    use super::*;

    #[test]
    fn test_model_job_new() {
        let job = ModelJob::new(
            "model_123".to_string(),
            "orders".to_string(),
            vec!["id".to_string(), "amount".to_string()],
        );

        assert_eq!(job.model_id, "model_123");
        assert_eq!(job.table, "orders");
        assert_eq!(job.fields.len(), 2);
        assert!(job.calculations.is_none());
        assert!(job.filters.is_none());
    }

    #[test]
    fn test_sort_spec_new() {
        let sort = SortSpec::new("amount".to_string(), true);

        assert_eq!(sort.column_name, "amount");
        assert_eq!(sort.sort_descending, true);
        assert!(sort.is_column_sort.is_none());
        assert!(sort.null_sort.is_none());
    }

    #[test]
    fn test_query_details_default() {
        let details = QueryDetails::default();
        assert!(details.model_job.is_none());

        let details2 = QueryDetails::new();
        assert!(details2.model_job.is_none());
    }
}

#[cfg(test)]
mod serialization_roundtrip_tests {
    use super::*;

    #[test]
    fn test_topic_response_roundtrip() {
        let topic_data = TopicData {
            name: "sales".to_string(),
            label: Some("Sales".to_string()),
            base_view_name: "sales_base".to_string(),
            views: vec![],
        };

        let response = TopicResponse {
            success: true,
            topic: topic_data,
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: TopicResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.success, true);
        assert_eq!(deserialized.topic.name, "sales");
    }

    #[test]
    fn test_view_data_with_fields() {
        let dim = DimensionData {
            field_name: "id".to_string(),
            view_name: "orders".to_string(),
            data_type: "number".to_string(),
            fully_qualified_name: "orders.id".to_string(),
            description: Some("Order ID".to_string()),
            ai_context: None,
            label: Some("ID".to_string()),
        };

        let measure = MeasureData {
            field_name: "total".to_string(),
            view_name: "orders".to_string(),
            data_type: "number".to_string(),
            fully_qualified_name: "orders.total".to_string(),
            description: Some("Total amount".to_string()),
            ai_context: Some("Sum of line items".to_string()),
            label: Some("Total".to_string()),
        };

        let view = ViewData {
            name: "orders".to_string(),
            dimensions: vec![dim],
            measures: vec![measure],
            filter_only_fields: vec!["status".to_string()],
        };

        let json = serde_json::to_string(&view).unwrap();
        let deserialized: ViewData = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.name, "orders");
        assert_eq!(deserialized.dimensions.len(), 1);
        assert_eq!(deserialized.measures.len(), 1);
        assert_eq!(deserialized.filter_only_fields.len(), 1);
    }

    #[test]
    fn test_models_response_with_pagination() {
        let page_info = PageInfo {
            has_next_page: true,
            has_previous_page: false,
            start_cursor: Some("start".to_string()),
            end_cursor: Some("end".to_string()),
            total_count: Some(50),
        };

        let record = ModelRecord {
            id: "1".to_string(),
            name: "model1".to_string(),
            label: None,
            description: None,
            created_at: None,
            updated_at: None,
        };

        let response = ModelsResponse {
            page_info,
            records: vec![record],
        };

        let json = serde_json::to_string(&response).unwrap();
        let deserialized: ModelsResponse = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.page_info.has_next_page, true);
        assert_eq!(deserialized.records.len(), 1);
    }
}
