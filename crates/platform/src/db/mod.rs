//! Database connection: pool initialization, auth-mode dispatch, RDS IAM
//! token refresher, and the task-router listener factory.
//!
//! Public surface is intentionally tiny — most users only need
//! [`establish_connection`] (the main pool) and
//! [`listener_factory_from_env`] (the agentic task router's dedicated
//! LISTEN connection). [`DatabaseAuthMode`] and [`IamConfig`] are also
//! exported so callers that need to validate env-var config before
//! attempting a connection (e.g. `oxy worker`) can share the canonical
//! parser without duplicating the logic.

pub(crate) mod auth_mode;
mod client;
pub(crate) mod iam;
mod listener;
mod pool_probe;

pub use auth_mode::{DatabaseAuthMode, IamConfig};
pub use client::establish_connection;
pub use listener::{listener_factory_from_env, listener_tls_verification_from_env};
