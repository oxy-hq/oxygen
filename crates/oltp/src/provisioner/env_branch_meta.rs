//! An empty environment branch keeps production's platform ledger.
//!
//! A schema-only branch has production's tables and none of its rows — and
//! `oxy_meta.schema_migrations` and `oxy_meta.app_migrations` are rows. Left
//! empty, the branch has all the structure those migrations built and no record
//! of having run them, so the first apply would run them again and fail on
//! `already exists`. [`OltpProvisioner::copy_meta_ledger`] copies the two tables
//! from production, so the branch knows which migrations its structure already
//! has.
//!
//! Idempotent (`ON CONFLICT DO NOTHING`): `LocalProvider` keeps `oxy_meta`
//! through its schema-only truncate, so there the rows are already present.
//! Only rows applied at or before the fork are copied, the same cut the control
//! plane's ledger copy makes (`production_ledger_at`).

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::entity::env_branches as rows;
use crate::entity::tenants as oltp_tenants;
use crate::platform::META_SCHEMA;
use crate::roles::quote_literal;

use super::branch_site::BranchSite;
use super::{OltpProvisioner, ProvisionerError, dsn_for};

/// Rows per `INSERT`, so a long ledger is several short statements.
const CHUNK: usize = 200;

/// One `oxy_meta` ledger table: how to read it as text from production and the
/// shape of the `INSERT` that puts a row on the branch.
struct MetaTable {
    name: &'static str,
    /// Columns, in the order the read returns them.
    columns: &'static [&'static str],
    /// A cast per column that makes the text literal typed.
    casts: &'static [&'static str],
    /// The primary key `ON CONFLICT` names.
    key: &'static str,
}

const META_TABLES: [MetaTable; 2] = [
    MetaTable {
        name: "schema_migrations",
        columns: &["file_path", "checksum", "revision_id", "applied_at"],
        casts: &["text", "text", "uuid", "timestamptz"],
        key: "file_path",
    },
    MetaTable {
        name: "app_migrations",
        columns: &["app_id", "version", "name", "checksum", "applied_at"],
        casts: &["uuid", "integer", "text", "text", "timestamptz"],
        key: "app_id, version",
    },
];

impl MetaTable {
    /// Production's rows applied at or before `fork_at`, every column as text.
    fn select_sql(&self, fork_at: DateTime<Utc>) -> String {
        let cols: Vec<String> = self.columns.iter().map(|c| format!("{c}::text")).collect();
        format!(
            "SELECT {} FROM {META_SCHEMA}.{} WHERE applied_at <= {}::timestamptz",
            cols.join(", "),
            self.name,
            quote_literal(&fork_at.to_rfc3339()),
        )
    }

    /// `INSERT … ON CONFLICT DO NOTHING` for `rows`, each a text per column.
    fn insert_sql(&self, rows: &[Vec<Option<String>>]) -> String {
        let tuples: Vec<String> = rows
            .iter()
            .map(|row| {
                let values: Vec<String> = row
                    .iter()
                    .zip(self.casts)
                    .map(|(v, cast)| match v {
                        Some(text) => format!("{}::{cast}", quote_literal(text)),
                        None => "NULL".to_string(),
                    })
                    .collect();
                format!("({})", values.join(", "))
            })
            .collect();
        format!(
            "INSERT INTO {META_SCHEMA}.{} ({}) VALUES {} ON CONFLICT ({}) DO NOTHING",
            self.name,
            self.columns.join(", "),
            tuples.join(", "),
            self.key,
        )
    }
}

impl OltpProvisioner {
    /// Copy production's `oxy_meta` migration ledgers (as of `fork_at`) onto
    /// the environment's branch.
    pub(super) async fn copy_meta_ledger(
        &self,
        tenant: &oltp_tenants::Model,
        row: &rows::Model,
        fork_at: DateTime<Utc>,
    ) -> Result<(), ProvisionerError> {
        let site = BranchSite::of_env(row).ok_or_else(|| uncut(tenant.org_id, row))?;
        let production = self.production_owner_dsn(tenant)?;
        let branch = self.branch_owner_dsn(tenant, &site)?;
        for table in &META_TABLES {
            let found = self
                .sql
                .query_text_rows(&production, &table.select_sql(fork_at))
                .await?;
            let batch: Vec<String> = found.chunks(CHUNK).map(|c| table.insert_sql(c)).collect();
            if !batch.is_empty() {
                self.sql.execute_batch(&branch, &batch).await?;
            }
        }
        Ok(())
    }

    /// Test seam: run the copy again on a branch whose `oxy_meta` rows a test
    /// removed, standing in for a Neon schema-only branch, which arrives
    /// without them. Reads the row as it is; takes no lock.
    #[doc(hidden)]
    pub async fn copy_env_meta_ledger_for_test(
        &self,
        key: &crate::env_branches::EnvBranchKey,
    ) -> Result<(), ProvisionerError> {
        let tenant = self.active_tenant(key.org_id).await?;
        let row = self
            .find_env_row(&tenant, key.app_id, &key.env_name)
            .await?
            .ok_or_else(|| ProvisionerError::EnvBranchNotCut {
                org_id: key.org_id,
                env_name: key.env_name.clone(),
            })?;
        self.copy_meta_ledger(&tenant, &row, row.fork_at.to_utc())
            .await
    }

    fn production_owner_dsn(
        &self,
        tenant: &oltp_tenants::Model,
    ) -> Result<String, ProvisionerError> {
        let password = self.owner_password(tenant, tenant.org_id)?;
        Ok(dsn_for(tenant, &tenant.owner_role, &password))
    }
}

/// A row asked for its site before the provider's answer was recorded.
pub(super) fn uncut(org_id: Uuid, row: &rows::Model) -> ProvisionerError {
    ProvisionerError::EnvBranchNotReady {
        org_id,
        env_name: row.env_name.clone(),
        status: row.status.as_str(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn the_read_is_text_and_stops_at_the_fork() {
        let at = Utc.with_ymd_and_hms(2026, 11, 2, 12, 0, 0).unwrap();
        let sql = META_TABLES[0].select_sql(at);
        assert_eq!(
            sql,
            "SELECT file_path::text, checksum::text, revision_id::text, applied_at::text \
             FROM oxy_meta.schema_migrations \
             WHERE applied_at <= '2026-11-02T12:00:00+00:00'::timestamptz"
        );
    }

    #[test]
    fn the_insert_types_every_value_and_never_overwrites() {
        let rows = vec![vec![
            Some("m'1.sql".to_string()),
            Some("abc".to_string()),
            None,
            Some("2026-11-02 12:00:00+00".to_string()),
        ]];
        let sql = META_TABLES[0].insert_sql(&rows);
        assert_eq!(
            sql,
            "INSERT INTO oxy_meta.schema_migrations (file_path, checksum, revision_id, applied_at) \
             VALUES ('m''1.sql'::text, 'abc'::text, NULL, '2026-11-02 12:00:00+00'::timestamptz) \
             ON CONFLICT (file_path) DO NOTHING"
        );
        let app = META_TABLES[1].insert_sql(&[vec![
            Some("00000000-0000-0000-0000-000000000001".into()),
            Some("3".into()),
            Some("n".into()),
            Some("c".into()),
            Some("2026-11-02 12:00:00+00".into()),
        ]]);
        assert!(
            app.ends_with("ON CONFLICT (app_id, version) DO NOTHING"),
            "{app}"
        );
        assert!(app.contains("'3'::integer"), "{app}");
    }
}
