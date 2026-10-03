//! Tenancy domain logic shared across bounded contexts.
//!
//! A domain crate in the sense of `internal-docs/domain-boundaries.md` rule S3:
//! it sits below `oxy-app` and every `api-*` surface, so a surface that needs
//! tenancy behavior imports it from here rather than reaching into another
//! surface or into `oxy-app`. It holds no routes and no handlers.

pub mod org_teams;
