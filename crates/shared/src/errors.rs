use std::sync::{PoisonError, TryLockError};

use crate::semantic_error::SemanticLayerError;
use async_openai::error::OpenAIError;
use axum::http::StatusCode;
use thiserror::Error;
use tokio::{sync::mpsc::error::SendError, task::JoinError};

#[derive(Error, Debug)]
pub enum OxyError {
    #[error("{0}")]
    ConfigurationError(String),
    #[error("{0}")]
    ArgumentError(String),
    #[error("{0}")]
    RuntimeError(String),
    #[error("{0}")]
    LLMError(String),
    #[error("{0}")]
    AgentError(String),
    #[error("{0}")]
    AnonymizerError(String),
    #[error("{0}")]
    SerializerError(String),
    #[error("{0}")]
    IOError(String),
    #[error("{0}")]
    DBError(String),
    /// A database call that never reached a usable connection — the statement
    /// was never sent. Split out of [`OxyError::DBError`] so the distinction
    /// survives the point where a `DbErr` becomes a string: the two need
    /// opposite people, and by the time a handler is rendering a 500 there is
    /// nothing typed left to ask. Classify with `oxy_platform::db::DbFailure`
    /// while the `DbErr` is still in hand, then carry the verdict in the
    /// variant.
    #[error("{0}")]
    DatabaseUnreachable(String),
    #[error("{0}")]
    Database(String),
    #[error("{0}")]
    SecretManager(String),
    #[error("{0:?}")]
    SecretNotFound(Option<String>),
    #[error("{0}")]
    AuthenticationError(String),
    #[error("{0}")]
    AuthorizationError(String),
    #[error("{0}")]
    ValidationError(String),
    #[error("Missing required filter: {filter}")]
    MissingRequiredFilter { filter: String },
    #[error("Unsupported filter: {filter}")]
    UnsupportedFilter { filter: String },
    #[error("Invalid type for filter '{filter}': expected {expected}, got {actual}. {details}")]
    InvalidFilterType {
        filter: String,
        expected: String,
        actual: String,
        details: String,
    },
    #[error(
        "Filter size limit exceeded for database '{database}': {size_bytes} bytes exceeds limit of {limit_bytes} bytes"
    )]
    FilterSizeLimitExceeded {
        database: String,
        size_bytes: usize,
        limit_bytes: usize,
    },
    #[error("{0}")]
    CryptographyError(String),
    #[error("{0}")]
    InitializationError(String),
    #[error("{0}")]
    JobError(String),
    #[error("{0}")]
    SerdeArrowError(#[from] serde_arrow::Error),
    #[error("{0}")]
    SemanticLayerError(#[from] SemanticLayerError),
    #[error("Error when calling {handle}:\n{msg}")]
    ToolCallError {
        call_id: String,
        handle: String,
        param: String,
        msg: String,
    },
}

impl OxyError {
    /// Get the error category for Sentry tagging
    pub fn category(&self) -> &'static str {
        match self {
            OxyError::ConfigurationError(_) => "configuration",
            OxyError::ArgumentError(_) => "argument",
            OxyError::RuntimeError(_) => "runtime",
            OxyError::LLMError(_) => "llm",
            OxyError::AgentError(_) => "agent",
            OxyError::AnonymizerError(_) => "anonymizer",
            OxyError::SerializerError(_) => "serializer",
            OxyError::IOError(_) => "io",
            OxyError::DBError(_) => "database",
            OxyError::DatabaseUnreachable(_) => "database_unreachable",
            OxyError::Database(_) => "database",
            OxyError::SecretManager(_) => "secret_manager",
            OxyError::SecretNotFound(_) => "secret_not_found",
            OxyError::AuthenticationError(_) => "authentication",
            OxyError::AuthorizationError(_) => "authorization",
            OxyError::ValidationError(_) => "validation",
            OxyError::MissingRequiredFilter { .. } => "filter_validation",
            OxyError::UnsupportedFilter { .. } => "filter_validation",
            OxyError::InvalidFilterType { .. } => "filter_validation",
            OxyError::FilterSizeLimitExceeded { .. } => "filter_validation",
            OxyError::CryptographyError(_) => "cryptography",
            OxyError::InitializationError(_) => "initialization",
            OxyError::JobError(_) => "job",
            OxyError::SerdeArrowError(_) => "serde_arrow",
            OxyError::ToolCallError { .. } => "tool_call",
            OxyError::SemanticLayerError(semantic_layer_error) => match semantic_layer_error {
                SemanticLayerError::VariableError(_) => "semantic_variable",
                SemanticLayerError::ConfigurationError(_) => "semantic_configuration",
                SemanticLayerError::ValidationError(_) => "semantic_validation",
                SemanticLayerError::ParsingError(_) => "semantic_parsing",
                SemanticLayerError::IOError(_) => "semantic_io",
            },
        }
    }

    /// Get the Sentry level for this error
    pub fn sentry_level(&self) -> sentry::Level {
        match self {
            OxyError::ConfigurationError(_) => sentry::Level::Warning,
            OxyError::ArgumentError(_) => sentry::Level::Warning,
            OxyError::RuntimeError(_) => sentry::Level::Error,
            OxyError::LLMError(_) => sentry::Level::Error,
            OxyError::AgentError(_) => sentry::Level::Error,
            OxyError::AnonymizerError(_) => sentry::Level::Error,
            OxyError::SerializerError(_) => sentry::Level::Error,
            OxyError::IOError(_) => sentry::Level::Error,
            OxyError::DBError(_) => sentry::Level::Error,
            OxyError::DatabaseUnreachable(_) => sentry::Level::Error,
            OxyError::Database(_) => sentry::Level::Error,
            OxyError::SecretManager(_) => sentry::Level::Error,
            OxyError::SecretNotFound(_) => sentry::Level::Warning,
            OxyError::AuthenticationError(_) => sentry::Level::Warning,
            OxyError::AuthorizationError(_) => sentry::Level::Warning,
            OxyError::ValidationError(_) => sentry::Level::Warning,
            OxyError::MissingRequiredFilter { .. } => sentry::Level::Warning,
            OxyError::UnsupportedFilter { .. } => sentry::Level::Warning,
            OxyError::InvalidFilterType { .. } => sentry::Level::Warning,
            OxyError::FilterSizeLimitExceeded { .. } => sentry::Level::Warning,
            OxyError::CryptographyError(_) => sentry::Level::Error,
            OxyError::InitializationError(_) => sentry::Level::Error,
            OxyError::JobError(_) => sentry::Level::Error,
            OxyError::SerdeArrowError(_) => sentry::Level::Error,
            OxyError::ToolCallError { .. } => sentry::Level::Error,
            OxyError::SemanticLayerError(_semantic_layer_error) => sentry::Level::Warning,
        }
    }

    /// Capture this error in Sentry with appropriate context
    pub fn capture_to_sentry(&self) {
        sentry::configure_scope(|scope| {
            scope.set_tag("error_category", self.category());
            scope.set_level(Some(self.sentry_level()));
        });

        if let OxyError::ToolCallError {
            call_id,
            handle,
            param,
            ..
        } = self
        {
            sentry::configure_scope(|scope| {
                scope.set_extra("call_id", call_id.clone().into());
                scope.set_extra("handle", handle.clone().into());
                scope.set_extra("param", param.clone().into());
            });
        }

        // Add filter-specific context for filter validation errors
        match self {
            OxyError::MissingRequiredFilter { filter } => {
                sentry::configure_scope(|scope| {
                    scope.set_extra("filter_name", filter.clone().into());
                });
            }
            OxyError::UnsupportedFilter { filter } => {
                sentry::configure_scope(|scope| {
                    scope.set_extra("filter_name", filter.clone().into());
                });
            }
            OxyError::InvalidFilterType {
                filter,
                expected,
                actual,
                details,
            } => {
                sentry::configure_scope(|scope| {
                    scope.set_extra("filter_name", filter.clone().into());
                    scope.set_extra("expected_type", expected.clone().into());
                    scope.set_extra("actual_type", actual.clone().into());
                    scope.set_extra("error_details", details.clone().into());
                });
            }
            _ => {}
        }

        sentry::capture_error(self);
    }
}

impl From<Box<dyn std::error::Error>> for OxyError {
    fn from(error: Box<dyn std::error::Error>) -> Self {
        OxyError::RuntimeError(error.to_string())
    }
}

impl From<anyhow::Error> for OxyError {
    fn from(error: anyhow::Error) -> Self {
        OxyError::RuntimeError(error.to_string())
    }
}

impl From<String> for OxyError {
    fn from(error: String) -> Self {
        OxyError::RuntimeError(error)
    }
}

impl<T> From<PoisonError<T>> for OxyError {
    fn from(error: PoisonError<T>) -> Self {
        OxyError::RuntimeError(format!("Failed to acquire lock: {error}"))
    }
}

impl<T> From<TryLockError<T>> for OxyError {
    fn from(error: TryLockError<T>) -> Self {
        OxyError::RuntimeError(format!("Failed to acquire lock: {error}"))
    }
}

impl From<serde_json::Error> for OxyError {
    fn from(error: serde_json::Error) -> Self {
        OxyError::SerializerError(error.to_string())
    }
}

impl<Event> From<SendError<Event>> for OxyError {
    fn from(error: SendError<Event>) -> Self {
        OxyError::RuntimeError(format!("Failed to send event: {error}"))
    }
}

impl From<JoinError> for OxyError {
    fn from(error: JoinError) -> Self {
        OxyError::RuntimeError(format!("Failed to join task: {error}"))
    }
}

impl From<std::io::Error> for OxyError {
    fn from(error: std::io::Error) -> Self {
        OxyError::IOError(error.to_string())
    }
}

impl From<OxyError> for StatusCode {
    fn from(error: OxyError) -> Self {
        tracing::error!("Error occurred: {}", error);
        error.capture_to_sentry();

        match error {
            OxyError::ConfigurationError(_) => StatusCode::BAD_REQUEST,
            OxyError::ArgumentError(_) => StatusCode::BAD_REQUEST,
            OxyError::RuntimeError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::LLMError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::AgentError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::AnonymizerError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::SerializerError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::IOError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::DBError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::DatabaseUnreachable(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::SecretManager(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::SecretNotFound(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::AuthenticationError(_) => StatusCode::UNAUTHORIZED,
            OxyError::AuthorizationError(_) => StatusCode::FORBIDDEN,
            OxyError::ValidationError(_) => StatusCode::BAD_REQUEST,
            OxyError::MissingRequiredFilter { .. } => StatusCode::BAD_REQUEST,
            OxyError::UnsupportedFilter { .. } => StatusCode::BAD_REQUEST,
            OxyError::InvalidFilterType { .. } => StatusCode::BAD_REQUEST,
            OxyError::FilterSizeLimitExceeded { .. } => StatusCode::BAD_REQUEST,
            OxyError::CryptographyError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::InitializationError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::JobError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::SerdeArrowError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::ToolCallError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
            OxyError::SemanticLayerError(_semantic_layer_error) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }
}

const CONTEXT_WINDOW_EXCEEDED_CODE: &str = "string_above_max_length";

impl From<OpenAIError> for OxyError {
    fn from(value: OpenAIError) -> Self {
        // Check if error message contains context window exceeded indication
        let error_str = format!("{value:?}");
        if error_str.contains(CONTEXT_WINDOW_EXCEEDED_CODE)
            || error_str.contains("context_length_exceeded")
        {
            return OxyError::LLMError(
                "Context window length exceeded. Shorten the prompt being sent to the LLM.".into(),
            );
        }
        OxyError::RuntimeError(format!("Error in completion request: {value:?}"))
    }
}
