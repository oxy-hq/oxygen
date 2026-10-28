//! # Omni Integration Crate
//!
//! This crate provides integration with Omni's semantic layer API, including:
//! - API client for interacting with Omni services
//! - Data models for Omni API responses and requests
//! - Error handling and resilience features

pub mod client;
pub mod error;
pub mod models;
pub mod resilience;

// Re-export main types for convenience
pub use client::OmniApiClient;
pub use error::OmniError;
pub use models::*;
pub use resilience::{ConnectionHealthChecker, RetryConfig, RetryPolicy, TimeoutWrapper};
