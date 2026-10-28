mod database;
mod environment;
mod manager;
mod storage;

pub use database::SecretsDatabaseStorage;
pub use manager::SecretsManager;
pub use storage::SecretsStorage;

// Re-export `OrgSecretsService` from `oxy-platform` under its legacy
// `oxy::adapters::secrets::OrgSecretsService` path. The implementation moved
// to platform so leaf integrations (airhouse, future ones) can use it without
// depending on the full `oxy` crate. The envelope-crypto helpers ride along
// for callers that need to seal small values under the master key without
// going through the org_secrets table, and so does the boot check that the
// master key opens what this database was sealed with (`key_check`).
pub use oxy_platform::secrets::{OrgSecretsService, envelope, key_check};
