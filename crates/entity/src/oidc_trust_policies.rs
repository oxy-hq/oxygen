//! `SeaORM` Entity for `oidc_trust_policies` — *a GitHub Actions run matching
//! these claims may act as this service account* (API-tokens design §3.4, §7).
//!
//! The repository is named by GitHub's numeric ids, resolved when the policy
//! is registered; `repository` is the `owner/repo` it had then, for display.
//! Foreign keys live in the migration
//! (`m20261002_000002_oidc_trust_policies`).

use sea_orm::entity::prelude::*;

/// The only provider there is.
pub const PROVIDER_GITHUB_ACTIONS: &str = "github_actions";

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "oidc_trust_policies")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub org_id: Uuid,
    /// `service_accounts.user_id` — the account a matching run acts as.
    pub service_account_id: Uuid,
    pub provider: String,
    pub repository_owner_id: i64,
    pub repository_id: i64,
    /// `owner/repo`. Display only; refreshed on each match.
    pub repository: String,
    /// `.github/workflows/release.yml`, or the full path of a reusable
    /// workflow in another repository.
    pub workflow_path: String,
    /// `None` = any environment, or none.
    pub environment: Option<String>,
    /// A glob on `ref`. `None` = any ref.
    pub ref_pattern: Option<String>,
    pub allow_self_hosted: bool,
    /// Audit only; never authority.
    pub created_by: Option<Uuid>,
    pub created_at: DateTimeWithTimeZone,
    pub last_used_at: Option<DateTimeWithTimeZone>,
    /// Set = the policy mints nothing.
    pub disabled_at: Option<DateTimeWithTimeZone>,
}

impl ActiveModelBehavior for ActiveModel {}
