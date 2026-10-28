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
    // With the key in KMS-sealed `server_keys`, the Job needs no variable at
    // all: open the row before a migration can seal anything. Only without the
    // variable: while it is set the Job uses it and never calls KMS, so its own
    // service account needs no KMS grant until then. The Job never seals a new
    // row; that stays with `oxy serve` / `oxy worker`, which compare it. A
    // fresh database has no `server_keys` yet, and nothing to seal either.
    if std::env::var(oxy::utils::ENCRYPTION_KEY_VAR).is_err() {
        crate::server::shared_state_check::open_master_key()
            .await
            .map_err(OxyError::RuntimeError)?;
    }
    crate::cli::commands::serve::run_database_migrations(false).await
}
