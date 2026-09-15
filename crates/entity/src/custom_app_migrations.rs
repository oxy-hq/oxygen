use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One `.sql` file a custom app has already applied to one of its own stores —
/// the `app_<writer>` schema in its org's OLTP Postgres, or in its workspace's
/// Airhouse. Written at promote by
/// `oxy_app::server::api::custom_apps_migrations`; never edited afterwards.
///
/// A row here is a *fact about the tenant database*, not a description of the
/// bundle: it survives the build that carried the file (`applied_by_build` goes
/// NULL when that build is GC'd) and it survives the file being deleted from a
/// later bundle. That asymmetry is the point — the ledger records what ran, and
/// what ran cannot be un-run by editing the repo.
///
/// See `migration::m20260905_000001_create_custom_app_migrations` for the
/// measured bug this replaces.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "custom_app_migrations")]
pub struct Model {
    /// Part of the composite PK. Per-app, so two apps may legitimately ship
    /// files with the same name and neither sees the other's ledger.
    #[sea_orm(primary_key, auto_increment = false)]
    pub app_id: Uuid,
    /// Which store the file ran against: `oltp` or `airhouse`. In the key
    /// because an app may ship `0001_init.sql` for both.
    #[sea_orm(primary_key, auto_increment = false)]
    pub store: String,
    /// The last part: the path RELATIVE to the bundle's declared migrations
    /// directory, so renaming that directory does not re-run everything.
    #[sea_orm(primary_key, auto_increment = false)]
    pub filename: String,
    /// Lowercase hex SHA-256 of the bytes that ran. Compared on every promote:
    /// a mismatch is a hard error, because it means someone edited a migration
    /// tenants have already applied.
    pub checksum: String,
    pub applied_at: DateTimeWithTimeZone,
    /// Which `app_builds` row carried the file. Provenance only — nullable, and
    /// nulled rather than cascaded when `gc_builds` reaps the build.
    pub applied_by_build: Option<Uuid>,
    #[sea_orm(
        belongs_to,
        from = "app_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub apps: BelongsTo<super::apps::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
