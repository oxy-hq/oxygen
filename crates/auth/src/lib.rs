//! Authentication and authorization for Oxy
//!
//! This crate provides:
//! - User authentication (built-in and external providers)
//! - API token management (`token`: format, storage, dispatch)
//! - Authorization middleware
//! - JWT token handling

pub mod api_key_infra;
pub mod app_publish_token_domain;
pub mod authenticator;
pub mod built_in;
pub mod caller_parts;
pub mod constants;
pub mod extractor;
pub mod frontline;
pub mod github_oidc;
pub mod middleware;
pub mod ports;
pub mod session_key;
pub mod token;
pub mod token_extend;
pub mod types;
pub mod user;
pub mod workload_identity;

// Re-export commonly used items
pub use api_key_infra::*;
pub use app_publish_token_domain::*;
pub use authenticator::*;
pub use built_in::*;
pub use caller_parts::caller_parts;
pub use extractor::*;
pub use middleware::*;
pub use token_extend::*;
pub use types::*;
pub use user::*;
