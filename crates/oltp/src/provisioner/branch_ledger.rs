//! The staging branch's migration ledger is a copy of production's, taken
//! when the branch's data is (previews P4b).
//!
//! A branch is cut — and reset — as a copy of production's database, so every
//! `CREATE TABLE` a custom app's migrations ran on production is already in
//! it. A ledger that started empty would make the first staging publish run
//! those files again and fail on `already exists`; so on every cut the
//! branch's `custom_app_migrations` rows (`branch:<provider id>`, store
//! `oltp`) are set to production's rows for the org's apps.
//!
//! **Read before the copy.** The migration apply commits a file's DDL before
//! it records the file, so a production row read before the provider copies
//! the database always describes DDL the copy holds. Read after, a file
//! applied in between would be recorded for a branch whose data lacks it, and
//! staging would skip it forever; read before, the worst case is a file the
//! copy holds that the ledger does not, which the next staging publish
//! re-runs and reports loudly (`already exists`) without failing anything.

use entity::custom_app_migrations as ledger;
use sea_orm::{ActiveValue, ColumnTrait, EntityTrait, QueryFilter, QuerySelect, TransactionTrait};
use uuid::Uuid;

use super::{OltpProvisioner, ProvisionerError};

use crate::branches::{LEDGER_PRODUCTION as PRODUCTION, LEDGER_STORE_OLTP as STORE_OLTP};

impl OltpProvisioner {
    /// Production's OLTP ledger rows for every app in `org_id` — what a copy
    /// of production's database taken after this read has applied.
    pub(super) async fn production_ledger(
        &self,
        org_id: Uuid,
    ) -> Result<Vec<ledger::Model>, ProvisionerError> {
        let apps: Vec<Uuid> = entity::apps::Entity::find()
            .select_only()
            .column(entity::apps::Column::Id)
            .filter(entity::apps::Column::OrgId.eq(org_id))
            .into_tuple()
            .all(&self.db)
            .await?;
        if apps.is_empty() {
            return Ok(Vec::new());
        }
        Ok(ledger::Entity::find()
            .filter(ledger::Column::Store.eq(STORE_OLTP))
            .filter(ledger::Column::Target.eq(PRODUCTION))
            .filter(ledger::Column::AppId.is_in(apps))
            .all(&self.db)
            .await?)
    }

    /// Make the ledger of branch `provider_branch_id` exactly `snapshot`
    /// (production's rows, [`Self::production_ledger`]), re-targeted at the
    /// branch — in one control-plane transaction, so a failure leaves the
    /// ledger as it was.
    pub(super) async fn seed_branch_ledger(
        &self,
        provider_branch_id: &str,
        snapshot: &[ledger::Model],
    ) -> Result<(), ProvisionerError> {
        let target = crate::branches::ledger_target(provider_branch_id);
        let txn = self.db.begin().await?;
        ledger::Entity::delete_many()
            .filter(ledger::Column::Store.eq(STORE_OLTP))
            .filter(ledger::Column::Target.eq(target.clone()))
            .exec(&txn)
            .await?;
        let copies = snapshot.iter().map(|row| ledger::ActiveModel {
            app_id: ActiveValue::Set(row.app_id),
            store: ActiveValue::Set(row.store.clone()),
            target: ActiveValue::Set(target.clone()),
            filename: ActiveValue::Set(row.filename.clone()),
            checksum: ActiveValue::Set(row.checksum.clone()),
            applied_at: ActiveValue::Set(row.applied_at),
            applied_by_build: ActiveValue::Set(row.applied_by_build),
        });
        if !snapshot.is_empty() {
            ledger::Entity::insert_many(copies).exec(&txn).await?;
        }
        txn.commit().await?;
        Ok(())
    }
}
