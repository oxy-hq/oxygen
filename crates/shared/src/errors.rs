use std::sync::{PoisonError, TryLockError};

use crate::semantic_error::SemanticLayerError;
use ::http::StatusCode;
use async_openai::error::OpenAIError;
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

/// Log this error as it leaves a handler, and report it to Sentry only when it
/// is **ours**.
///
/// An authentication failure is the CALLER's credential, so it is not reported.
/// The issue it filed was unactionable by construction — the fix is always
/// "present a credential that admits" — and every junk token was one more event
/// against the quota. It also looked new every release: nothing pins the
/// fingerprint of a `capture_error` event, which keeps the stack
/// `attach_stacktrace` gives it at send time, and
/// `oxy::sentry_config::normalize_tracing_event` deliberately leaves an event
/// carrying an exception alone.
///
/// `/external/api` has always avoided this: its refusal answers through
/// `oxy_auth::middleware::unauthorized`, which never comes through here, so a
/// `CORS: *` surface cannot be used to fill the quota. `/api` is the same
/// refusal on a surface with less traffic, and had no reason to be louder.
///
/// Still logged, once, at `warn`: a 401 the caller asked for is not an
/// incident, and the detail belongs in the platform logs where the caller can
/// be identified. Deliberately the ONLY line — `auth_middleware` has no log of
/// its own, because the two together said the same thing twice.
///
/// A free function rather than a method, and not an `if` inside the conversion
/// below, because the metrics gate holds both shapes still: that conversion is
/// one exhaustive match recorded at cyclomatic 27 against a limit of 15, and
/// `impl OxyError` is recorded at wmc 69 against 60. Growing either fails the
/// gate, and neither reads better for carrying this.
fn log_on_the_way_out(error: &OxyError) {
    if matches!(error, OxyError::AuthenticationError(_)) {
        tracing::warn!("Authentication failed: {error}");
        return;
    }
    tracing::error!("Error occurred: {}", error);
    error.capture_to_sentry();
}

impl From<OxyError> for StatusCode {
    fn from(error: OxyError) -> Self {
        log_on_the_way_out(&error);

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Why this file knows about Sentry at all: a 401 the caller asked for
    /// files no issue, while a fault of ours still does.
    ///
    /// Asserted through the real conversion rather than a predicate. The
    /// conversion is what every handler and `oxy_auth::middleware` actually
    /// call, and the failure this pins is a predicate that agrees while the
    /// call site reports anyway.
    #[test]
    fn an_authentication_failure_files_no_issue_and_a_server_fault_still_does() {
        let refusals = sentry::test::with_captured_events(|| {
            for message in [
                "Invalid API key",
                "No API key found in headers",
                "Missing or invalid authentication header",
            ] {
                assert_eq!(
                    StatusCode::from(OxyError::AuthenticationError(message.to_string())),
                    StatusCode::UNAUTHORIZED,
                    "{message}"
                );
            }
        });
        assert!(refusals.is_empty(), "{refusals:?}");

        // Not a restatement: the same conversion still reports what IS ours.
        let ours = sentry::test::with_captured_events(|| {
            assert_eq!(
                StatusCode::from(OxyError::DBError("pool timed out".to_string())),
                StatusCode::INTERNAL_SERVER_ERROR
            );
        });
        assert_eq!(ours.len(), 1, "{ours:?}");
    }
}
