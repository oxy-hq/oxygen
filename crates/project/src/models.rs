use axum::http::StatusCode;
use serde::{Deserialize, Deserializer, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

fn deserialize_optional_u64_from_string<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de::Error;

    let value: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    match value {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => {
            if s.is_empty() {
                Ok(None)
            } else {
                s.parse::<u64>().map(Some).map_err(D::Error::custom)
            }
        }
        Some(serde_json::Value::Number(n)) => n
            .as_u64()
            .map(Some)
            .ok_or_else(|| D::Error::custom("expected a non-negative integer")),
        Some(other) => Err(D::Error::custom(format!(
            "expected string or number, got {other}"
        ))),
    }
}

#[derive(Deserialize, ToSchema)]
pub struct WorkspaceFormData {
    pub name: String,
    pub r#type: String,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct PostgresConfig {
    pub host: Option<String>,
    pub port: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub password_var: Option<String>,
    pub database: Option<String>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct RedshiftConfig {
    pub host: Option<String>,
    pub port: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub password_var: Option<String>,
    pub database: Option<String>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct MysqlConfig {
    pub host: Option<String>,
    pub port: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub password_var: Option<String>,
    pub database: Option<String>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct ClickHouseConfig {
    pub host: Option<String>,
    pub user: Option<String>,
    pub password: Option<String>,
    pub password_var: Option<String>,
    pub database: Option<String>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct BigQueryConfig {
    pub key: Option<String>,
    /// The secret holding the service-account key's JSON contents. When set,
    /// no key file is written.
    pub key_json_var: Option<String>,
    pub dataset: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional_u64_from_string")]
    pub dry_run_limit: Option<u64>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct DuckDBConfig {
    pub file_search_path: Option<String>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct SnowflakeConfig {
    pub account: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub password_var: Option<String>,
    pub warehouse: Option<String>,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub role: Option<String>,
    pub private_key_path: Option<String>,
    /// The secret holding the private key's contents (`auth_mode:
    /// "private_key"`); preferred over `private_key_path`.
    pub private_key_var: Option<String>,
    pub auth_mode: Option<String>, // "password", "browser", or "private_key"
}

#[derive(Deserialize, ToSchema, Debug)]
pub struct WarehouseConfig {
    pub r#type: String,
    pub name: Option<String>,
    pub config: serde_json::Value,
}

impl WarehouseConfig {
    pub fn get_postgres_config(&self) -> Result<PostgresConfig, StatusCode> {
        self.parse_config("postgres")
    }

    pub fn get_redshift_config(&self) -> Result<RedshiftConfig, StatusCode> {
        self.parse_config("redshift")
    }

    pub fn get_mysql_config(&self) -> Result<MysqlConfig, StatusCode> {
        self.parse_config("mysql")
    }

    pub fn get_clickhouse_config(&self) -> Result<ClickHouseConfig, StatusCode> {
        self.parse_config("clickhouse")
    }

    pub fn get_bigquery_config(&self) -> Result<BigQueryConfig, StatusCode> {
        self.parse_config("bigquery")
    }

    pub fn get_duckdb_config(&self) -> Result<DuckDBConfig, StatusCode> {
        self.parse_config("duckdb")
    }

    pub fn get_snowflake_config(&self) -> Result<SnowflakeConfig, StatusCode> {
        self.parse_config("snowflake")
    }

    fn parse_config<T: for<'de> Deserialize<'de>>(&self, warehouse: &str) -> Result<T, StatusCode> {
        serde_json::from_value::<T>(self.config.clone()).map_err(|e| {
            tracing::error!("Failed to deserialize {warehouse} warehouse config: {e}");
            StatusCode::BAD_REQUEST
        })
    }
}

#[derive(Deserialize, ToSchema)]
pub struct WarehousesFormData {
    pub warehouses: Vec<WarehouseConfig>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ModelVendor {
    OpenAI,
    Google,
    Anthropic,
    Ollama,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct OpenAIModelConfig {
    pub model_ref: Option<String>,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct GoogleModelConfig {
    pub model_ref: Option<String>,
    pub api_key: Option<String>,
    pub project_id: Option<String>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct AnthropicModelConfig {
    pub model_ref: Option<String>,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
}

#[derive(Deserialize, Serialize, ToSchema, Debug, Clone)]
pub struct OllamaModelConfig {
    pub model_ref: Option<String>,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct ModelConfig {
    pub vendor: ModelVendor,
    pub name: Option<String>,
    pub config: serde_json::Value,
}

impl ModelConfig {
    pub fn get_openai_config(&self) -> OpenAIModelConfig {
        serde_json::from_value::<OpenAIModelConfig>(self.config.clone()).unwrap_or({
            OpenAIModelConfig {
                model_ref: None,
                api_key: None,
                api_url: None,
            }
        })
    }

    pub fn get_google_config(&self) -> GoogleModelConfig {
        serde_json::from_value::<GoogleModelConfig>(self.config.clone()).unwrap_or({
            GoogleModelConfig {
                model_ref: None,
                api_key: None,
                project_id: None,
            }
        })
    }

    pub fn get_anthropic_config(&self) -> AnthropicModelConfig {
        serde_json::from_value::<AnthropicModelConfig>(self.config.clone()).unwrap_or({
            AnthropicModelConfig {
                model_ref: None,
                api_key: None,
                api_url: None,
            }
        })
    }

    pub fn get_ollama_config(&self) -> OllamaModelConfig {
        serde_json::from_value::<OllamaModelConfig>(self.config.clone()).unwrap_or({
            OllamaModelConfig {
                model_ref: None,
                api_key: None,
                api_url: None,
            }
        })
    }
}

#[derive(Deserialize, ToSchema)]
pub struct ModelsFormData {
    pub models: Vec<ModelConfig>,
}

#[derive(Deserialize, ToSchema)]
pub struct ToolConfig {
    pub r#type: String,
    pub name: String,
    pub description: String,
    pub database: Option<String>,
}

#[derive(Deserialize, ToSchema)]
pub struct AgentConfig {
    pub name: String,
    pub model: String,
    pub system_instructions: String,
    pub description: Option<String>,
    pub public: Option<bool>,
    pub tools: Option<Vec<ToolConfig>>,
}

#[derive(Deserialize, ToSchema)]
pub struct GitHubData {
    pub namespace_id: Uuid,
    pub repo_id: i64,
    pub branch: String,
}

#[derive(Deserialize, ToSchema)]
pub struct CreateWorkspaceRequest {
    pub workspace: WorkspaceFormData,
    pub openai_api_key: Option<String>,
    pub warehouses: Option<WarehousesFormData>,
    pub model: Option<ModelsFormData>,
    pub agent: Option<AgentConfig>,
    pub github: Option<GitHubData>,
}
