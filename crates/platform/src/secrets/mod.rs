//! Org-scoped encrypted secret store + AES key loader.
//!
//! [`OrgSecretsService`] is the persistence path for per-org secrets used
//! across the oxy platform (Slack tokens, Airhouse passwords, GitHub PATs).
//! [`get_encryption_key`] reads the AES-256 master key from the
//! `OXY_ENCRYPTION_KEY` env var with a dev-friendly file fallback.
//! [`key_check`] refuses a boot whose key cannot open what this database's
//! values were sealed with. [`master_key`] keeps the master key in Postgres,
//! sealed by KMS, in a Kubernetes namespace that has the KMS alias for it.

mod encryption;
pub mod envelope;
pub mod key_check;
pub mod master_key;
mod org_secrets;

pub use encryption::{
    ENCRYPTION_KEY_VAR, MasterKeySource, encryption_key_source, forbid_key_file_creation,
    get_encryption_key, install_master_key, installed_master_key, master_key_text_for_signing,
    parse_encryption_key,
};
pub use org_secrets::OrgSecretsService;
