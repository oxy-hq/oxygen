//! Identity domain logic shared across surfaces (`internal-docs/domain-boundaries.md`
//! S3, S9): below `oxy-app` and every `api-*` surface, above `oxy-app-core`.
//! No routes, no entities, no migrator.
//!
//! - [`token_lifecycle`]: the personal-token lifecycle audit. Written by the
//!   token routes and by the custom-app sandbox sweep, two surfaces that may
//!   not import each other; too context-owned for `oxy-app-core` (S9).

pub mod token_lifecycle;
