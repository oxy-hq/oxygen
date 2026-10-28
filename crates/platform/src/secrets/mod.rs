//! Org-scoped encrypted secret store + AES key loader.
//!
//! [`OrgSecretsService`] is the persistence path for per-org secrets used
//! across the oxy platform (Slack tokens, Airhouse passwords, GitHub PATs).
//! [`get_encryption_key`] reads the AES-256 master key from the
//! `OXY_ENCRYPTION_KEY` env var with a dev-friendly file fallback.

mod encryption;
pub mod envelope;
mod org_secrets;

pub use encryption::{ENCRYPTION_KEY_VAR, get_encryption_key, parse_encryption_key};
pub use org_secrets::OrgSecretsService;
