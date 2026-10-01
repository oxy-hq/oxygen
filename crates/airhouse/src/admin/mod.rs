//! Airhouse Admin API HTTP client.
//!
//! Wraps the `/admin/v1/{tenants,users,service-accounts,tenants/.../tokens}`
//! endpoints exposed by an Airhouse deployment. Used by the tenant +
//! per-user provisioners and the SA-backed token broker.

mod client;
mod error;
mod types;

pub use client::{AirhouseAdminClient, Capabilities};
pub use error::AirhouseError;
pub use types::{
    CatalogIndexState, CatalogIndexesResponse, CreatedServiceAccount, EphemeralCredential,
    ServiceAccountRecord, TenantRecord, TokenAuth, UserRecord, UserRole,
};
