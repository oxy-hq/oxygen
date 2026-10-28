//! The row in Postgres.

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use super::{MASTER_KEY_ROW, MasterKeyStore};
use crate::secrets::encryption::ENCRYPTION_KEY_VAR;
use crate::secrets::key_check;

/// The row in `server_keys`.
pub struct PostgresStore<'a, C> {
    pub db: &'a C,
}

pub(super) fn db_error(e: impl std::fmt::Display) -> String {
    format!("Could not read or write server_keys row `{MASTER_KEY_ROW}`: {e}")
}

impl<C: ConnectionTrait> MasterKeyStore for PostgresStore<'_, C> {
    async fn read(&self) -> Result<Option<Vec<u8>>, String> {
        let row = self
            .db
            .query_one_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "SELECT secret FROM server_keys WHERE name = $1",
                [MASTER_KEY_ROW.into()],
            ))
            .await
            .map_err(db_error)?;
        row.map(|r| r.try_get::<Vec<u8>>("", "secret"))
            .transpose()
            .map_err(db_error)
    }

    async fn insert_if_absent(&self, row: &[u8]) -> Result<(), String> {
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DatabaseBackend::Postgres,
                "INSERT INTO server_keys (name, secret) VALUES ($1, $2) ON CONFLICT (name) DO NOTHING",
                [MASTER_KEY_ROW.into(), row.to_vec().into()],
            ))
            .await
            .map_err(db_error)?;
        Ok(())
    }

    async fn verify_variable(&self, key: &[u8; 32]) -> Result<(), String> {
        key_check::verify_key(self.db, key, ENCRYPTION_KEY_VAR).await
    }
}
