//! What an org's staging branch IS to the rest of Oxy: how old, whom a reset
//! hits, and which migration-ledger rows belong to it.
//!
//! Luong's ruling on env design §11 #16 (2026-09-29): the branch is **manual**.
//! Created by `oxyc oltp provision --branch staging` (one per org), reset only
//! on demand (org-level, confirmed, naming every app it affects), never on a
//! timer; past [`STALE_AFTER_DAYS`] the API reports it `stale` so a console can
//! warn; deleted on the org-deletion path. The age is what the 30 days meant
//! under the MSA: production rows deleted under the 30-day commitment live on
//! in a branch cut before the deletion, until someone resets it.

use chrono::{DateTime, FixedOffset};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};
use serde::Serialize;
use uuid::Uuid;

use crate::entity::branches::{self as oltp_branches, Entity as OltpBranches, OltpBranch};
use crate::entity::roles::{self as oltp_roles, Entity as OltpRoles, WriterKind};
use crate::entity::tenants::{self as oltp_tenants, Entity as OltpTenants};

/// Past this many days a branch is reported `stale`. Reported, never acted on.
pub const STALE_AFTER_DAYS: i64 = 30;

/// `custom_app_migrations.store` for a file applied to an app's OLTP schema.
/// `oxy-app`'s ledger (`custom_apps_migrations`) keys by this same constant.
pub const LEDGER_STORE_OLTP: &str = "oltp";

/// `custom_app_migrations.target` for a file applied to production's database.
pub const LEDGER_PRODUCTION: &str = "production";

/// `custom_app_migrations.target` for OLTP files applied to this branch.
///
/// Must equal `MigrationTarget::Branch(id).as_key()` in `oxy-app` — the ledger
/// already keys by target, with `branch:<provider branch id>` reserved for
/// exactly this (a test there pins the two together). A cut or a reset sets
/// these rows to a copy of production's, since the branch's data is a copy of
/// production's (`provisioner::branch_ledger`); a delete clears them.
pub fn ledger_target(provider_branch_id: &str) -> String {
    format!("branch:{provider_branch_id}")
}

/// How old a branch's data is, and whether that is past the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BranchAge {
    pub age_days: i64,
    pub stale: bool,
}

/// Age counts from the last cut — creation, or the last reset, which re-copies
/// production and so restarts the clock. Whole days, floored: a branch is not
/// "31 days old" until 31 full days have passed, and `stale` means strictly
/// more than [`STALE_AFTER_DAYS`].
pub fn age_of(
    now: DateTime<FixedOffset>,
    created_at: DateTime<FixedOffset>,
    last_reset_at: Option<DateTime<FixedOffset>>,
) -> BranchAge {
    let cut = last_reset_at.unwrap_or(created_at);
    let age_days = (now - cut).num_days().max(0);
    BranchAge {
        age_days,
        stale: age_days > STALE_AFTER_DAYS,
    }
}

/// An app whose staging data lives in the branch — what a reset throws away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AffectedApp {
    pub app_id: Uuid,
    pub slug: String,
    pub name: String,
    /// Its `app_<writer>` schema.
    pub schema: String,
}

/// Every app in the org with an OLTP writer, in slug order.
///
/// "Has a writer" rather than "is in the org": an app without `ctx.oltp` has
/// nothing in the branch, and listing it would bury the ones that do. The
/// match is the platform's own derivation (`app_writer_name`), so an app
/// counts exactly when `ctx.oltp` would resolve to one of these writers.
pub async fn affected_apps(
    db: &DatabaseConnection,
    org_id: Uuid,
    tenant_row_id: Uuid,
) -> Result<Vec<AffectedApp>, DbErr> {
    let writers: Vec<String> = OltpRoles::find()
        .filter(oltp_roles::Column::TenantRowId.eq(tenant_row_id))
        .filter(oltp_roles::Column::WriterKind.eq(WriterKind::App))
        .all(db)
        .await?
        .into_iter()
        .map(|r| r.writer_name)
        .collect();
    let apps = entity::apps::Entity::find()
        .filter(entity::apps::Column::OrgId.eq(org_id))
        .all(db)
        .await?;
    Ok(match_affected(
        apps.into_iter().map(|a| (a.id, a.slug, a.name)),
        &writers,
    ))
}

/// An Airway pipeline's `raw_<source>` schema in the branch. A reset discards
/// whatever a staging run wrote there too, so it is listed beside the apps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AffectedPipeline {
    pub source: String,
    pub schema: String,
}

/// Every pipeline writer in the tenant, in schema order.
pub async fn affected_pipelines(
    db: &DatabaseConnection,
    tenant_row_id: Uuid,
) -> Result<Vec<AffectedPipeline>, DbErr> {
    let mut out: Vec<AffectedPipeline> = OltpRoles::find()
        .filter(oltp_roles::Column::TenantRowId.eq(tenant_row_id))
        .filter(oltp_roles::Column::WriterKind.eq(WriterKind::Pipeline))
        .all(db)
        .await?
        .into_iter()
        .map(|r| AffectedPipeline {
            source: r.writer_name,
            schema: r.schema_name,
        })
        .collect();
    out.sort_by(|a, b| a.schema.cmp(&b.schema));
    Ok(out)
}

/// The pure half of [`affected_apps`].
pub(crate) fn match_affected(
    apps: impl IntoIterator<Item = (Uuid, String, String)>,
    writer_names: &[String],
) -> Vec<AffectedApp> {
    let mut out: Vec<AffectedApp> = apps
        .into_iter()
        .filter_map(|(app_id, slug, name)| {
            let writer = crate::schema::app_writer_name(&slug)?;
            writer_names.contains(&writer).then(|| AffectedApp {
                app_id,
                schema: format!("app_{writer}"),
                slug,
                name,
            })
        })
        .collect();
    out.sort_by(|a, b| a.slug.cmp(&b.slug));
    out
}

/// One cut of a branch: the row, the provider's branch id, and when its data
/// was copied (`last_reset_at`, else `created_at`). A reset changes the cut;
/// a re-cut under a new id changes the id too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchCut {
    pub row_id: Uuid,
    pub provider_branch_id: String,
    pub cut_at: DateTime<FixedOffset>,
}

impl BranchCut {
    /// The cut `row` describes now.
    pub fn of(row: &oltp_branches::Model) -> Self {
        Self {
            row_id: row.id,
            provider_branch_id: row.provider_branch_id.clone(),
            cut_at: row.last_reset_at.unwrap_or(row.created_at),
        }
    }
}

/// Whether the branch is still `cut`, and `active` — read `FOR SHARE`, so a
/// reset's status change waits for the caller's transaction to finish. A
/// caller recording what it applied to a branch (the staging migration
/// apply) runs this in the same transaction as the record: a branch reset or
/// re-cut since the caller connected has discarded what it applied, and a
/// row recorded now would make staging skip that file on the new copy.
pub async fn still_current<C: sea_orm::ConnectionTrait>(
    conn: &C,
    cut: &BranchCut,
) -> Result<bool, DbErr> {
    use sea_orm::QuerySelect;
    let row = OltpBranches::find_by_id(cut.row_id)
        .lock_shared()
        .one(conn)
        .await?;
    Ok(row.is_some_and(|row| {
        row.status == oltp_branches::BranchStatus::Active && BranchCut::of(&row) == *cut
    }))
}

/// The org's tenant and its `kind` branch, whichever exist.
pub async fn find(
    db: &DatabaseConnection,
    org_id: Uuid,
    kind: OltpBranch,
) -> Result<(Option<oltp_tenants::Model>, Option<oltp_branches::Model>), DbErr> {
    let Some(tenant) = OltpTenants::find()
        .filter(oltp_tenants::Column::OrgId.eq(org_id))
        .one(db)
        .await?
    else {
        return Ok((None, None));
    };
    let branch = OltpBranches::find()
        .filter(oltp_branches::Column::TenantRowId.eq(tenant.id))
        .filter(oltp_branches::Column::Kind.eq(kind))
        .one(db)
        .await?;
    Ok((Some(tenant), branch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone, Utc};

    fn at(days: i64) -> DateTime<FixedOffset> {
        (Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).unwrap() + Duration::days(days)).into()
    }

    #[test]
    fn thirty_days_is_not_stale_and_thirty_one_is() {
        let created = at(0);
        assert_eq!(
            age_of(at(30), created, None),
            BranchAge {
                age_days: 30,
                stale: false
            }
        );
        assert_eq!(
            age_of(at(31), created, None),
            BranchAge {
                age_days: 31,
                stale: true
            }
        );
        // Floored: 30 days and 23 hours is still 30.
        let almost = at(31) - Duration::hours(1);
        assert!(!age_of(almost, created, None).stale);
    }

    #[test]
    fn a_reset_restarts_the_clock() {
        let age = age_of(at(90), at(0), Some(at(85)));
        assert_eq!(age.age_days, 5);
        assert!(
            !age.stale,
            "a branch reset five days ago holds five-day-old data"
        );
    }

    #[test]
    fn a_clock_behind_the_row_reads_zero_not_negative() {
        assert_eq!(age_of(at(0), at(1), None).age_days, 0);
    }

    #[test]
    fn affected_apps_are_the_ones_whose_derived_writer_exists() {
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let got = match_affected(
            [
                (a, "store-ops".to_string(), "Store Ops".to_string()),
                (b, "brochure".to_string(), "Brochure".to_string()),
                (c, "bookings".to_string(), "Bookings".to_string()),
            ],
            &["store_ops".to_string(), "bookings".to_string()],
        );
        let slugs: Vec<&str> = got.iter().map(|x| x.slug.as_str()).collect();
        assert_eq!(
            slugs,
            ["bookings", "store-ops"],
            "sorted, writer-less app left out"
        );
        assert_eq!(
            got[1].schema, "app_store_ops",
            "the derived schema, hyphen mapped"
        );
    }

    #[test]
    fn the_ledger_target_is_the_reserved_branch_key() {
        assert_eq!(ledger_target("br-cold-sky"), "branch:br-cold-sky");
    }
}
