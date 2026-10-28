//! Data models for Looker API responses and requests

use serde::{Deserialize, Serialize};

// Authentication Models

/// Response from the Looker login endpoint (`POST /api/4.0/login`).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoginResponse {
    /// The access token to use for API requests
    pub access_token: String,
    /// The type of token (typically "Bearer")
    pub token_type: String,
    /// Number of seconds until the token expires
    pub expires_in: u64,
}

// LookML Model Response Models

/// Represents a LookML model returned from the Looker API.
///
/// A LookML model is a collection of explores that define how data can be queried.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LookmlModel {
    /// The unique name of the model
    pub name: String,
    /// Human-readable label for the model
    pub label: Option<String>,
    /// The LookML project this model belongs to
    pub project_name: Option<String>,
    /// List of explores available in this model
    #[serde(default)]
    pub explores: Vec<LookmlModelNavExplore>,
}

/// Summary information about an explore within a LookML model.
///
/// This is the lightweight version returned when listing models.
/// Use `LookmlModelExplore` for full explore metadata.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LookmlModelNavExplore {
    /// The unique name of the explore
    pub name: String,
    /// Human-readable label for the explore
    pub label: Option<String>,
    /// Description of what this explore contains
    pub description: Option<String>,
    /// Whether this explore is hidden from users
    pub hidden: Option<bool>,
}

/// Detailed metadata for a LookML explore.
///
/// Retrieved from `GET /api/4.0/lookml_models/{model}/explores/{explore}`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LookmlModelExplore {
    /// The unique name of the explore
    pub name: String,
    /// Human-readable label for the explore
    pub label: Option<String>,
    /// Description of what this explore contains
    pub description: Option<String>,
    /// The base view name for this explore (used as the `view` parameter in queries)
    pub view_name: Option<String>,
    pub fields: Option<ExploreFields>,
    /// The LookML source file defining this explore
    pub source_file: Option<String>,
    pub sql_table_name: Option<String>,
}

/// Collection of fields available in an explore, organized by type.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ExploreFields {
    /// Dimension fields (attributes/columns)
    #[serde(default)]
    pub dimensions: Vec<LookmlModelExploreField>,
    /// Measure fields (aggregations)
    #[serde(default)]
    pub measures: Vec<LookmlModelExploreField>,
    /// Filter-only fields
    #[serde(default)]
    pub filters: Vec<LookmlModelExploreField>,
    #[serde(default)]
    pub parameters: Vec<LookmlModelExploreField>,
}

/// A field (dimension, measure, filter, or parameter) within an explore.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LookmlModelExploreField {
    /// The fully-qualified field name (e.g., "view_name.field_name")
    pub name: String,
    /// Human-readable label for the field
    pub label: Option<String>,
    /// Description of what this field represents
    pub description: Option<String>,
    /// The category of field: "dimension", "measure", "filter", or "parameter"
    #[serde(rename = "category")]
    pub field_type: Option<String>,
    /// The view this field belongs to
    pub view: Option<String>,
    /// The SQL expression for this field
    pub sql: Option<String>,
    /// The data type (string, number, date, yesno, etc.)
    #[serde(rename = "type")]
    pub type_: Option<String>,
    /// Whether this field is hidden from users
    pub hidden: Option<bool>,
    /// For filter fields, the dimension to suggest values from
    pub suggest_dimension: Option<String>,
    /// For filter fields, the explore to suggest values from
    pub suggest_explore: Option<String>,
}

// Query Models

/// Request body for running an inline query via `POST /api/4.0/queries/run/{result_format}`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InlineQueryRequest {
    /// The name of the LookML model to query
    pub model: String,
    /// The base view name to query (e.g., from explore `view_name`)
    pub view: String,
    /// List of field names to include in the results
    pub fields: Vec<String>,
    /// Filter conditions as field name to filter expression mappings
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<std::collections::HashMap<String, String>>,
    /// Looker filter expression for complex OR conditions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_expression: Option<String>,
    /// List of sort expressions (e.g. "orders.created_date desc")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sorts: Option<Vec<String>>,
    /// Maximum number of rows to return (-1 for unlimited)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub query_timezone: Option<String>,
    /// Fields to pivot on
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pivots: Option<Vec<String>>,
    /// Fields to fill with previous values
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill_fields: Option<Vec<String>>,
}

/// Response from a Looker query execution.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueryResponse {
    /// The query result data as a list of row objects
    #[serde(default)]
    pub data: Vec<std::collections::HashMap<String, serde_json::Value>>,
    /// Metadata about the fields in the response
    #[serde(default)]
    pub fields: std::collections::HashMap<String, QueryFieldMetadata>,
    /// The generated SQL query (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct QueryFieldMetadata {
    /// The field name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Human-readable label
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The data type
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
}

/// A saved query object returned from `POST /api/4.0/queries`.
///
/// This represents a query that has been created but not yet executed.
/// Use the `id` field with `run_query()` to execute the query.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Query {
    /// Unique identifier for the query
    pub id: i64,
    /// The name of the LookML model
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The name of the explore (view)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub view: Option<String>,
    /// List of field names in the query
    #[serde(default)]
    pub fields: Vec<String>,
    /// Filter conditions
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filters: Option<std::collections::HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filter_expression: Option<String>,
    /// Sort fields
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sorts: Option<Vec<String>>,
    /// Row limit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    /// The generated SQL (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_login_response_deserialize() {
        let json = r#"{
            "access_token": "abc123",
            "token_type": "Bearer",
            "expires_in": 3600
        }"#;

        let response: LoginResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.access_token, "abc123");
        assert_eq!(response.token_type, "Bearer");
        assert_eq!(response.expires_in, 3600);
    }

    #[test]
    fn test_lookml_model_deserialize() {
        let json = r#"{
            "name": "ecommerce",
            "label": "E-Commerce",
            "project_name": "my_project",
            "explores": [
                {
                    "name": "orders",
                    "label": "Orders",
                    "description": "Order data",
                    "hidden": false
                }
            ]
        }"#;

        let model: LookmlModel = serde_json::from_str(json).unwrap();
        assert_eq!(model.name, "ecommerce");
        assert_eq!(model.label, Some("E-Commerce".to_string()));
        assert_eq!(model.explores.len(), 1);
        assert_eq!(model.explores[0].name, "orders");
    }

    #[test]
    fn test_lookml_model_explore_deserialize() {
        let json = r#"{
            "name": "orders",
            "label": "Orders",
            "description": "Order analytics",
            "source_file": "orders.view.lkml",
            "sql_table_name": "public.orders",
            "fields": {
                "dimensions": [
                    {
                        "name": "orders.id",
                        "label": "ID",
                        "type": "number",
                        "view": "orders"
                    }
                ],
                "measures": [
                    {
                        "name": "orders.count",
                        "label": "Count",
                        "type": "count",
                        "view": "orders"
                    }
                ],
                "filters": [],
                "parameters": []
            }
        }"#;

        let explore: LookmlModelExplore = serde_json::from_str(json).unwrap();
        assert_eq!(explore.name, "orders");
        assert_eq!(explore.sql_table_name, Some("public.orders".to_string()));

        let fields = explore.fields.unwrap();
        assert_eq!(fields.dimensions.len(), 1);
        assert_eq!(fields.dimensions[0].name, "orders.id");
        assert_eq!(fields.measures.len(), 1);
        assert_eq!(fields.measures[0].name, "orders.count");
    }

    #[test]
    fn test_explore_field_deserialize() {
        let json = r#"{
            "name": "orders.created_date",
            "label": "Created Date",
            "description": "When the order was created",
            "category": "dimension",
            "view": "orders",
            "sql": "${TABLE}.created_at",
            "type": "date",
            "hidden": false
        }"#;

        let field: LookmlModelExploreField = serde_json::from_str(json).unwrap();
        assert_eq!(field.name, "orders.created_date");
        assert_eq!(field.field_type, Some("dimension".to_string()));
        assert_eq!(field.type_, Some("date".to_string()));
        assert_eq!(field.hidden, Some(false));
    }

    #[test]
    fn test_inline_query_request_serialize() {
        let mut filters = std::collections::HashMap::new();
        filters.insert(
            "orders.created_date".to_string(),
            "last 30 days".to_string(),
        );

        let request = InlineQueryRequest {
            model: "ecommerce".to_string(),
            view: "orders".to_string(),
            fields: vec!["orders.id".to_string(), "orders.total".to_string()],
            filters: Some(filters),
            filter_expression: None,
            sorts: Some(vec!["orders.created_date desc".to_string()]),
            limit: Some(100),
            query_timezone: None,
            pivots: None,
            fill_fields: None,
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"model\":\"ecommerce\""));
        assert!(json.contains("\"view\":\"orders\""));
        assert!(json.contains("\"limit\":100"));
    }

    #[test]
    fn test_query_response_deserialize() {
        let json = r#"{
            "data": [
                {"orders.id": 1, "orders.total": 99.99},
                {"orders.id": 2, "orders.total": 149.99}
            ],
            "fields": {
                "orders.id": {"name": "orders.id", "label": "ID", "type": "number"},
                "orders.total": {"name": "orders.total", "label": "Total", "type": "number"}
            },
            "sql": "SELECT id, total FROM orders"
        }"#;

        let response: QueryResponse = serde_json::from_str(json).unwrap();
        assert_eq!(response.data.len(), 2);
        assert_eq!(response.fields.len(), 2);
        assert_eq!(
            response.sql,
            Some("SELECT id, total FROM orders".to_string())
        );
    }
}
