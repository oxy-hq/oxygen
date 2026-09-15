use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use utoipa::ToSchema;

use crate::{
    config::model::SemanticFilter,
    exec_types::{
        ReferenceKind, Table, Usage,
        event::{ArtifactKind, SandboxInfo, Step},
    },
    types::tool_params::OmniQueryParams,
    utils::get_file_stem,
};

pub mod agent;
pub mod block;
pub mod content;
pub mod message;
pub mod pagination;
pub mod run;
pub mod task;
pub mod tool_params;

// Re-export commonly used types
pub use agent::{ArtifactInfo, AskAgentResponse, LogItem, LogType};
pub use message::Message;

#[derive(Serialize, Debug, Clone, ToSchema)]
#[serde(tag = "type")]
pub enum ContainerKind {
    #[serde(rename = "workflow")]
    Workflow { r#ref: String },
    #[serde(rename = "agent")]
    Agent { r#ref: String },
    #[serde(rename = "execute_sql")]
    ExecuteSQL { database: String },
    #[serde(rename = "task")]
    Task { name: String },
    #[serde(rename = "artifact")]
    Artifact {
        artifact_id: String,
        kind: String,
        title: String,
        is_verified: bool,
    },
}

impl std::fmt::Display for ContainerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ContainerKind::Workflow { r#ref } => {
                write!(f, "⏳Running workflow: {}", get_file_stem(r#ref))
            }
            ContainerKind::Agent { r#ref } => write!(f, "⏳Starting {}", get_file_stem(r#ref)),
            ContainerKind::ExecuteSQL { database } => {
                write!(f, "⏳Execute SQL on Database: {database}")
            }
            ContainerKind::Task { name } => write!(f, "⏳Starting {name}"),
            ContainerKind::Artifact {
                kind,
                title,
                is_verified,
                artifact_id,
            } => write!(
                f,
                ":::artifact{{id={artifact_id} kind={kind} title={title} verified={is_verified}}}\n:::\n"
            ),
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct ExecuteSQL {
    pub database: String,
    pub sql_query: String,
    pub result: Vec<Vec<String>>,
    pub is_result_truncated: bool,
}

#[derive(Serialize, Deserialize, ToSchema, JsonSchema, Clone, Debug, Hash)]
pub struct SemanticQueryOrder {
    pub field: String,
    pub direction: String,
}

#[derive(Serialize, Deserialize, ToSchema, JsonSchema, Clone, Debug)]
pub struct SemanticQueryExport {
    pub path: String,
    pub format: String,
}

/// Time granularity options for time dimension queries
#[derive(Serialize, Deserialize, ToSchema, JsonSchema, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum TimeGranularity {
    Year,
    Quarter,
    Month,
    Week,
    Day,
    Hour,
    Minute,
    Second,
}

#[derive(Serialize, Deserialize, ToSchema, JsonSchema, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(untagged)]
pub enum DateRange {
    /// Relative expression: "last week", "this month", "from 7 days ago to now"
    Relative(String),
    /// Array of 1-2 dates: ["2023-01-01"] or ["2023-01-01", "2023-12-31"]
    Dates(Vec<String>),
}

impl DateRange {
    /// Validates that the date range has valid structure (1-2 dates for Dates variant)
    pub fn validate(&self) -> Result<(), String> {
        match self {
            DateRange::Relative(_) => Ok(()),
            DateRange::Dates(dates) => {
                if dates.is_empty() {
                    Err("Date range must have at least 1 date".to_string())
                } else if dates.len() > 2 {
                    Err(format!(
                        "Date range must have at most 2 dates, got {}",
                        dates.len()
                    ))
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Creates a single-date range (same start and end)
    pub fn single(date: String) -> Self {
        DateRange::Dates(vec![date])
    }

    /// Creates a date range from start to end
    pub fn range(from: String, to: String) -> Self {
        DateRange::Dates(vec![from, to])
    }

    /// Creates a relative date range expression
    pub fn relative(expr: impl Into<String>) -> Self {
        DateRange::Relative(expr.into())
    }
}

/// Time dimension for temporal queries with granularity and date range
#[derive(Serialize, Deserialize, ToSchema, JsonSchema, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TimeDimension {
    /// Dimension name in format: <view_name>.<dimension_name>
    pub dimension: String,
    /// Time granularity for grouping (year, month, day, etc.)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granularity: Option<TimeGranularity>,
}

impl TimeDimension {
    /// Validates the time dimension structure
    pub fn validate(&self) -> Result<(), String> {
        if self.dimension.is_empty() {
            return Err("Time dimension name cannot be empty".to_string());
        }

        Ok(())
    }
}

// Reusable set of semantic query parameters (mirrors task definition inputs)
#[derive(Serialize, Deserialize, ToSchema, JsonSchema, Clone, Debug, Default)]
pub struct SemanticQueryParams {
    #[serde(default)]
    pub topic: Option<String>,
    #[schemars(
        description = "List of measures to include in the query. Format: <view_name>.<measure_name>"
    )]
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub measures: Vec<String>,
    #[schemars(
        description = "List of dimensions to include in the query. Format: <view_name>.<dimension_name>"
    )]
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub dimensions: Vec<String>,
    #[schemars(
        description = "List of time dimensions with granularity and date range. Can only use with dimensions of type time or datetime. Format: <view_name>.<dimension_name>"
    )]
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub time_dimensions: Vec<TimeDimension>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub filters: Vec<SemanticFilter>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub orders: Vec<SemanticQueryOrder>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    /// Variables for semantic model expressions (e.g. table names, column names, filters)
    #[schemars(
        description = "Variables to resolve in semantic model expressions. Use {{variables.variable_name}} syntax in semantic definitions."
    )]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<HashMap<String, Value>>,
}

impl std::hash::Hash for SemanticQueryParams {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.topic.hash(state);
        self.measures.hash(state);
        self.dimensions.hash(state);
        for td in &self.time_dimensions {
            td.hash(state);
        }
        for filter in &self.filters {
            filter.hash(state);
        }
        for order in &self.orders {
            order.hash(state);
        }
        self.limit.hash(state);
        self.offset.hash(state);
        // Variables affect query results, so include them in hash
        if let Some(variables) = &self.variables {
            for (key, value) in variables {
                key.hash(state);
                value.to_string().hash(state); // Hash the JSON string representation
            }
        }
    }
}

#[derive(Serialize, Deserialize, Clone, ToSchema, Debug, Hash)]
pub struct SemanticQuery {
    pub database: String,
    pub sql_query: String,
    pub result: Vec<Vec<String>>,
    pub error: Option<String>,
    pub validation_error: Option<String>,
    pub sql_generation_error: Option<String>,
    pub is_result_truncated: bool,
    pub topic: Option<String>,
    pub dimensions: Vec<String>,
    pub measures: Vec<String>,
    pub time_dimensions: Vec<TimeDimension>,
    pub filters: Vec<SemanticFilter>,
    pub orders: Vec<SemanticQueryOrder>,
    pub limit: Option<u64>,
    pub offset: Option<u64>,
}

impl SemanticQuery {
    pub fn get_semantic_query_json(&self) -> String {
        let semantic_query_params = SemanticQueryParams {
            topic: self.topic.clone(),
            measures: self.measures.clone(),
            dimensions: self.dimensions.clone(),
            filters: self.filters.clone(),
            orders: self.orders.clone(),
            limit: self.limit,
            offset: self.offset,
            variables: None,
            time_dimensions: self.time_dimensions.clone(),
        };
        serde_json::to_string_pretty(&semantic_query_params)
            .unwrap_or_else(|_| "Failed to serialize SemanticQueryParams".to_string())
    }
}

#[derive(Serialize, Deserialize, Clone, ToSchema, Debug)]
pub struct OmniQuery {
    pub result: Vec<Vec<String>>,
    pub is_result_truncated: bool,
    pub topic: String,
    pub fields: Vec<String>,
    pub limit: Option<u64>,
    pub sorts: Option<std::collections::HashMap<String, String>>,
}

#[derive(Serialize, Deserialize, Clone, ToSchema, Debug)]
pub struct LookerQuery {
    pub result: Vec<Vec<String>>,
    pub is_result_truncated: bool,
    pub integration: String,
    pub model: String,
    pub explore: String,
    pub fields: Vec<String>,
    pub filters: Option<std::collections::HashMap<String, String>>,
    pub sorts: Option<Vec<String>>,
    pub limit: Option<i64>,
    pub sql: String,
}

impl std::hash::Hash for LookerQuery {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.result.hash(state);
        self.is_result_truncated.hash(state);
        self.integration.hash(state);
        self.model.hash(state);
        self.explore.hash(state);
        self.fields.hash(state);

        match &self.filters {
            Some(filters) => {
                1u8.hash(state);
                let mut entries = filters.iter().collect::<Vec<_>>();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                for (key, value) in entries {
                    key.hash(state);
                    value.hash(state);
                }
            }
            None => 0u8.hash(state),
        }

        self.sorts.hash(state);
        self.limit.hash(state);
        self.sql.hash(state);
    }
}

#[derive(Serialize, ToSchema)]
#[serde(tag = "type", content = "value")]
pub enum ArtifactValue {
    #[serde(rename = "log_item")]
    LogItem(LogItem),
    #[serde(rename = "content")]
    Content(String),
    #[serde(rename = "execute_sql")]
    ExecuteSQL(ExecuteSQL),
    #[serde(rename = "semantic_query")]
    SemanticQuery(SemanticQuery),
    #[serde(rename = "omni_query")]
    OmniQuery(OmniQuery),
    #[serde(rename = "looker_query")]
    LookerQuery(LookerQuery),
    #[serde(rename = "sandbox_info")]
    SandboxInfo(SandboxInfo),
}

#[derive(Serialize, Deserialize, ToSchema, Debug)]
#[serde(tag = "type", content = "value")]
pub enum ArtifactContent {
    #[serde(rename = "workflow")]
    Workflow { r#ref: String, output: Vec<LogItem> },
    #[serde(rename = "agent")]
    Agent { r#ref: String, output: String },
    #[serde(rename = "execute_sql")]
    ExecuteSQL {
        database: String,
        sql_query: String,
        result: Vec<Vec<String>>,
        is_result_truncated: bool,
    },
    #[serde(rename = "semantic_query")]
    SemanticQuery(SemanticQuery),
    #[serde(rename = "omni_query")]
    OmniQuery(OmniArtifactContent),
    #[serde(rename = "looker_query")]
    LookerQuery(LookerArtifactContent),
    #[serde(rename = "sandbox_info")]
    SandboxInfo(SandboxInfo),
}

#[derive(Serialize, Deserialize, ToSchema, Debug)]
pub struct OmniArtifactContent {
    pub result: Vec<Vec<String>>,
    pub is_result_truncated: bool,
    pub topic: String,
    pub sql: String,
    pub fields: Vec<String>,
    pub limit: Option<u64>,
    pub sorts: Option<std::collections::HashMap<String, String>>,
}

#[derive(Serialize, Deserialize, ToSchema, Debug)]
pub struct LookerArtifactContent {
    pub result: Vec<Vec<String>>,
    pub is_result_truncated: bool,
    pub model: String,
    pub explore: String,
    pub sql: String,
    pub fields: Vec<String>,
    pub filters: Option<std::collections::HashMap<String, String>>,
    pub sorts: Option<Vec<String>>,
    pub limit: Option<i64>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum AnswerContent {
    Text {
        content: String,
    },
    /// Begin a reasoning span. Renderers open the affordance now (Slack
    /// drops to native streaming cursor; Web opens a collapsible panel)
    /// and don't have to buffer to detect the first chunk.
    ReasoningStarted {
        id: String,
    },
    ReasoningChunk {
        id: String,
        delta: String,
    },
    ReasoningDone {
        id: String,
    },
    /// A chart artifact emitted by the visualize tool. The `chart_src`
    /// is the canonical filename (e.g. `<uuid>.json`) — the renderer is
    /// responsible for turning it into a public URL or surface-specific
    /// rendering.
    Chart {
        chart_src: String,
    },
    ArtifactStarted {
        id: String,
        title: String,
        is_verified: bool,
        kind: ArtifactKind,
    },
    ArtifactValue {
        id: String,
        value: ArtifactValue,
    },
    ArtifactDone {
        id: String,
        error: Option<String>,
    },
    Error {
        message: String,
    },
    Usage {
        usage: Usage,
    },
    DataApp {
        file_path: String,
    },
    StepStarted {
        step: Step,
    },
    StepFinished {
        step_id: String,
        error: Option<String>,
    },
}

#[derive(Serialize, ToSchema)]
pub struct AnswerStream {
    pub content: AnswerContent,
    pub references: Vec<ReferenceKind>,
    pub is_error: bool,
    pub step: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", content = "value")]
pub enum Content {
    Text(String),
    SQL(String),
    Table(Table),
    OmniQuery(OmniQueryParams),
    LookerQuery(LookerQuery),
    SandboxInfo(SandboxInfo),
    SemanticQuery(SemanticQuery),
    /// A persisted reference to a chart file. `to_markdown` round-trips this
    /// to the canonical `:chart{chart_src=…}` directive so stored thread
    /// markdown remains parseable by the web markdown plugins.
    Chart(String),
}

impl Content {
    fn to_markdown(&self) -> String {
        match self {
            Content::Text(text) => text.clone(),
            Content::SQL(sql) => format!("\n```sql\n{sql}\n```\n"),
            Content::Table(table) => table.to_markdown(),
            Content::OmniQuery(omni_query_params) => {
                let json = serde_json::to_string_pretty(omni_query_params)
                    .unwrap_or_else(|_| "Failed to serialize OmniQueryParams".to_string());
                format!("\n```json\n{json}\n```\n")
            }
            Content::LookerQuery(_) => "".to_string(),
            Content::SandboxInfo(sandbox_info) => {
                format!(
                    "[{} App Preview]({})",
                    sandbox_info.kind, sandbox_info.preview_url
                )
            }
            Content::SemanticQuery(_) => "".to_string(),
            Content::Chart(chart_src) => format!(":chart{{chart_src={chart_src}}}"),
        }
    }
}

#[cfg(test)]
mod content_to_markdown_tests {
    use super::*;

    #[test]
    fn chart_serializes_to_canonical_directive() {
        let content = Content::Chart("abc-123.json".to_string());
        assert_eq!(content.to_markdown(), ":chart{chart_src=abc-123.json}");
    }
}

#[cfg(test)]
mod answer_content_serde_tests {
    use super::*;

    #[test]
    fn reasoning_started_round_trip() {
        let v = AnswerContent::ReasoningStarted {
            id: "r1".to_string(),
        };
        let json = serde_json::to_value(&v).expect("serialize");
        assert_eq!(json["type"], "reasoning_started");
        assert_eq!(json["id"], "r1");
    }

    #[test]
    fn reasoning_chunk_round_trip() {
        let v = AnswerContent::ReasoningChunk {
            id: "r1".to_string(),
            delta: "hello".to_string(),
        };
        let json = serde_json::to_value(&v).expect("serialize");
        assert_eq!(json["type"], "reasoning_chunk");
        assert_eq!(json["id"], "r1");
        assert_eq!(json["delta"], "hello");
    }

    #[test]
    fn reasoning_done_round_trip() {
        let v = AnswerContent::ReasoningDone {
            id: "r1".to_string(),
        };
        let json = serde_json::to_value(&v).expect("serialize");
        assert_eq!(json["type"], "reasoning_done");
    }

    #[test]
    fn chart_round_trip() {
        let v = AnswerContent::Chart {
            chart_src: "abc.json".to_string(),
        };
        let json = serde_json::to_value(&v).expect("serialize");
        assert_eq!(json["type"], "chart");
        assert_eq!(json["chart_src"], "abc.json");
    }
}

#[derive(Serialize, Clone, Debug)]
#[serde(untagged)]
pub enum BlockValue {
    Content {
        content: Content,
    },
    Children {
        #[serde(flatten)]
        kind: ContainerKind,
        children: Vec<Block>,
    },
}

#[derive(Serialize, Clone, Debug)]
pub struct Block {
    pub id: String,
    #[serde(flatten)]
    pub value: Box<BlockValue>,
}

impl Block {
    pub fn is_artifact(&self) -> bool {
        matches!(self.value.as_ref(), BlockValue::Children { kind, .. } if matches!(kind, ContainerKind::Artifact { .. }))
    }
    pub fn container(id: String, kind: ContainerKind) -> Self {
        Block {
            id,
            value: Box::new(BlockValue::Children {
                kind,
                children: vec![],
            }),
        }
    }
    pub fn content(id: String, content: Content) -> Self {
        Block {
            id,
            value: Box::new(BlockValue::Content { content }),
        }
    }
    fn details_opener(summary: &str) -> String {
        format!("<details>\n<summary>{summary}</summary>\n")
    }
    fn details_closer() -> String {
        "</details>".to_string()
    }
    fn artifacts_opener(
        id: &str,
        kind: &str,
        title: &str,
        is_verified: bool,
        fences_count: usize,
    ) -> String {
        format!(
            "{}artifact{{id={} kind={} title={} is_verified={}}}",
            ":".repeat(fences_count),
            id,
            kind,
            title,
            is_verified
        )
    }
    fn artifacts_closer(fences_count: usize) -> String {
        ":".repeat(fences_count)
    }
    pub fn container_opener_closer(
        kind: &ContainerKind,
        max_artifact_fences: &mut usize,
    ) -> (String, String) {
        match kind {
            ContainerKind::Workflow { .. } => (
                Block::details_opener(&kind.to_string()),
                Block::details_closer(),
            ),
            ContainerKind::Agent { .. } => (
                Block::details_opener(&kind.to_string()),
                Block::details_closer(),
            ),
            ContainerKind::ExecuteSQL { .. } => (
                Block::details_opener(&kind.to_string()),
                Block::details_closer(),
            ),
            ContainerKind::Task { .. } => (
                Block::details_opener(&kind.to_string()),
                Block::details_closer(),
            ),
            ContainerKind::Artifact {
                artifact_id,
                kind,
                title,
                is_verified,
            } => {
                let result = (
                    Block::artifacts_opener(
                        artifact_id,
                        kind,
                        title,
                        *is_verified,
                        *max_artifact_fences,
                    ),
                    Block::artifacts_closer(*max_artifact_fences),
                );
                *max_artifact_fences = max_artifact_fences.saturating_sub(1);
                if *max_artifact_fences < 3 {
                    *max_artifact_fences = 3;
                }
                result
            }
        }
    }
    pub fn to_markdown(&self, max_artifact_fences: usize) -> String {
        let mut next_fences = max_artifact_fences;
        match self.value.as_ref() {
            BlockValue::Content { content } => content.to_markdown(),
            BlockValue::Children { kind, children } => {
                let mut markdown = String::new();
                let (block_opener, block_closer) =
                    Block::container_opener_closer(kind, &mut next_fences);
                markdown.push('\n');
                markdown.push_str(&block_opener);
                markdown.push('\n');
                for child in children {
                    markdown.push_str(&child.to_markdown(next_fences));
                }
                markdown.push_str("\n\n");
                markdown.push_str(&block_closer);
                markdown.push('\n');
                markdown
            }
        }
    }
    pub fn as_log_items(&self) -> Vec<LogItem> {
        let mut log_items = vec![];
        match self.value.as_ref() {
            BlockValue::Content { content } => match content {
                Content::Text(text) => log_items.push(LogItem::info(text.clone())),
                Content::SQL(sql) => {
                    log_items.push(LogItem::info(format!("Query:\n```sql\n{sql}\n```\n")))
                }
                Content::Table(table) => {
                    log_items.push(LogItem::info(
                        format!("Result:\n{}\n", table.to_markdown(),),
                    ));
                }
                Content::OmniQuery(omni_query_params) => {
                    let json = serde_json::to_string_pretty(omni_query_params)
                        .unwrap_or_else(|_| "Failed to serialize OmniQueryParams".to_string());
                    log_items.push(LogItem::info(format!(
                        "Omni Query:\n```json\n{json}\n```\n"
                    )));
                }
                Content::LookerQuery(looker_query_params) => {
                    let json = serde_json::to_string_pretty(looker_query_params)
                        .unwrap_or_else(|_| "Failed to serialize LookerQueryParams".to_string());
                    log_items.push(LogItem::info(format!(
                        "Looker Query:\n```json\n{json}\n```\n"
                    )));
                }
                Content::SandboxInfo(SandboxInfo { preview_url, kind }) => {
                    log_items.push(LogItem::info(format!("[{}]({})", kind, preview_url)));
                }
                Content::SemanticQuery(semantic_query) => {
                    let json = serde_json::to_string_pretty(semantic_query)
                        .unwrap_or_else(|_| "Failed to serialize SemanticQuery".to_string());
                    log_items.push(LogItem::info(format!(
                        "Semantic Query:\n```json\n{json}\n```\n"
                    )));
                }
                Content::Chart(chart_src) => {
                    log_items.push(LogItem::info(format!("Chart: {chart_src}")));
                }
            },
            BlockValue::Children { kind, children } => match kind {
                ContainerKind::Workflow { r#ref } => {
                    log_items.push(LogItem::info(format!(
                        "⏳Running Workflow: {}",
                        get_file_stem(r#ref)
                    )));
                    log_items.extend(children.iter().flat_map(|child| child.as_log_items()));
                }
                ContainerKind::Agent { r#ref } => {
                    log_items.push(LogItem::info(format!(
                        "⏳Starting {}",
                        get_file_stem(r#ref)
                    )));
                    log_items.extend(children.iter().flat_map(|child| child.as_log_items()));
                }
                ContainerKind::ExecuteSQL { database } => {
                    log_items.push(LogItem::info(format!(
                        "⏳Execute SQL on Database: {database}"
                    )));
                    log_items.extend(children.iter().flat_map(|child| child.as_log_items()));
                }
                ContainerKind::Task { name } => {
                    log_items.push(LogItem::info(format!("⏳Starting {name}")));
                    log_items.extend(children.iter().flat_map(|child| child.as_log_items()));
                }
                ContainerKind::Artifact { .. } => {
                    log_items.push(LogItem::info(kind.to_string()));
                }
            },
        }
        log_items
    }
}
