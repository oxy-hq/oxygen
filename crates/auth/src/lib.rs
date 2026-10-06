//! Authentication and authorization for Oxy
//!
//! This crate provides:
//! - User authentication (built-in and external providers)
//! - API key and token management (`token`: format, storage, dispatch)
//! - Authorization middleware
//! - JWT token handling

pub mod api_key_domain;
pub mod api_key_extend;
pub mod api_key_infra;
pub mod app_publish_token_domain;
pub mod authenticator;
pub mod built_in;
pub mod constants;
pub mod extractor;
pub mod frontline;
pub mod github_oidc;
pub mod middleware;
pub mod token;
pub mod types;
pub mod user;

// Re-export commonly used items
pub use api_key_domain::*;
pub use api_key_extend::*;
pub use api_key_infra::*;
pub use app_publish_token_domain::*;
pub use authenticator::*;
pub use built_in::*;
pub use extractor::*;
pub use middleware::*;
pub use types::*;
pub use user::*;
