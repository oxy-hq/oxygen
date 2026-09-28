//! Per-org OLTP Postgres.
//!
//! Oxy can read a customer's data and write derived rows to a warehouse; it has
//! nowhere to put *transactional* data. This crate provisions one Postgres per
//! **org**, with one schema per **writer** (an Airway pipeline or a custom app),
//! so custom apps and ELT pipelines have a real OLTP store.
//!
//! # Shape
//!
//! Deliberately parallel to the `airhouse` crate, which already solved "provision a
//! per-tenant data plane and broker credentials into it":
//!
//! | airhouse | here |
//! | --- | --- |
//! | `airhouse_tenants` (per workspace) | [`entity::tenants`] (per org) |
//! | `TenantProvisioner` | [`provisioner::OltpProvisioner`] |
//! | `AirhouseAdminClient` | [`provider::OltpProvider`] |
//! | ephemeral SA-minted credentials | durable per-writer roles ([`entity::roles`]) |
//!
//! That last row is the one real divergence: Neon roles are **durable** — the API
//! creates a role and returns its password once — so passwords are sealed at rest
//! rather than minted per request. Rotation is an explicit operation, not a TTL.
//!
//! # Providers
//!
//! [`provider::OltpProvider`] is a trait shaped by the Neon REST API, with three
//! implementations: [`provider::NeonProvider`] (real), [`provider::LocalProvider`]
//! (a database per org on a local cluster), and [`provider::MockProvider`] for
//! tests. DDL runs through [`sql::TenantSqlExecutor`], so the schema/role model
//! is unit-testable without a live Postgres — and is additionally verified
//! against a real one (`tests/integration/grants.rs`) and against real Neon
//! (`tests/integration/neon_live.rs`, opt-in).
//!
//! **Every role is minted in SQL, never through the provider API.** Neon's API
//! makes each role it creates a member of `neon_superuser`, which would hand a
//! per-writer role the run of the database. [`roles::assert_confined_sql`] refuses to
//! proceed if a role turns out to hold membership it should not, raising
//! SQLSTATE `OXY01`.
//!
//! Design: `internal-docs/per-org-oltp-postgres.md`.

pub mod api;
pub mod config;
pub mod connect;
pub mod entity;
pub mod local_seed;
pub mod migration;
pub mod migrator;
pub mod platform;
pub mod provider;
pub mod provisioner;
pub mod resolver;
pub mod roles;
pub mod schema;
pub mod sql;

pub use config::{OltpConfig, OltpRuntimeConfig, ProviderKind};
pub use provider::{OltpProvider, ProviderError};
pub use provisioner::{OltpProvisioner, ProvisionerError};
pub use schema::{GrantLevel, WriterRef};
