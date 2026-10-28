//! # Looker Integration Crate
//!
//! This crate provides integration with Looker's API, including:
//! - API client for interacting with Looker services
//! - Data models for Looker API responses and requests
//! - Error handling

pub mod client;
pub mod error;
pub mod models;

// Re-export main types for convenience
pub use client::{AccessToken, LookerApiClient, LookerAuthConfig};
pub use error::LookerError;
pub use models::{InlineQueryRequest, QueryResponse};
