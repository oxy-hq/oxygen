use oxy_shared::errors::OxyError;

/// Run ALL database migrations to the latest version.
///
/// Delegates to [`crate::cli::commands::serve::run_database_migrations`], which
/// runs every domain migrator (core SeaORM + runtime + analytics + workflow +
/// airway + airhouse + cameras) under a process-serialising Postgres advisory
/// lock — the SAME complete set `oxy serve` runs. The previous implementation
/// ran only the core `Migrator`, so a `migrate`-as-Job left the domain tables
/// uncreated.
///
/// `oxy migrate` is the dedicated one-shot entrypoint — e.g. a Helm
/// pre-install/pre-upgrade Job — so the serving pods can boot with
/// `OXY_SKIP_MIGRATIONS=1` instead of each racing the migrator.
///
/// That Job is never the single process, and the `oxy-app` chart wires
/// `encryptionKey.secretName` into the pods but not into the Job. A migration that seals a
/// value (the camera `device_secret` backfill does) must then fail, naming the
/// variable, rather than seal it under a key file minted inside the Job.
pub async fn migrate() -> Result<(), OxyError> {
    oxy::utils::forbid_key_file_creation("oxy migrate");
    crate::cli::commands::serve::run_database_migrations(false).await
}
