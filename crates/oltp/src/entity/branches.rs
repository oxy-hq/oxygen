//! `oltp_branches` — an org's non-production branches of its OLTP database.
//!
//! One row per `(tenant, kind)`, and the only kind is `staging` (env design
//! §4.3: one branch per **org**, because OLTP is per org). Beside
//! `oltp_tenants` and shaped like it: where to connect, and the branch owner's
//! sealed password. The roles *inside* the branch carry their own sealed
//! credentials in [`super::branch_roles`].
//!
//! Created by hand (`oxyc oltp provision --branch staging`), reset only on
//! demand, deleted with the tenant — the 2026-09-29 ruling on §11 #16.

use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// Which branch. The API spelling (`staging`) is also the stored one.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, EnumIter, DeriveActiveEnum,
)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(16))")]
#[serde(rename_all = "snake_case")]
pub enum OltpBranch {
    /// The org's staging copy, which every app's staging environment shares.
    #[sea_orm(string_value = "staging")]
    Staging,
}

impl OltpBranch {
    pub fn as_str(&self) -> &'static str {
        match self {
            OltpBranch::Staging => "staging",
        }
    }

    /// The name the provider sees. Derived, never chosen — which is what lets
    /// `create_branch` adopt a branch of this name as its own half-finished
    /// work rather than someone else's.
    pub fn provider_name(&self) -> &'static str {
        match self {
            OltpBranch::Staging => "oxy-staging",
        }
    }

    /// Parse the API spelling. Unknown names are refused rather than mapped:
    /// a typo must not reset the wrong thing.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "staging" => Some(OltpBranch::Staging),
            _ => None,
        }
    }
}

impl std::fmt::Display for OltpBranch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, EnumIter, DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "String(StringLen::N(32))")]
#[serde(rename_all = "snake_case")]
pub enum BranchStatus {
    /// Cut, and every credential for it minted. The only state a resolver
    /// hands out a connection in.
    #[sea_orm(string_value = "active")]
    Active,
    /// Recorded, credentials not yet all minted — a provision that failed part
    /// way. The next provision finishes it.
    #[sea_orm(string_value = "provisioning")]
    Provisioning,
    /// A reset is in flight or failed part way. Held here until it completes so
    /// nothing pairs a half-reset branch with credentials the reset replaced.
    #[sea_orm(string_value = "resetting")]
    Resetting,
}

impl BranchStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            BranchStatus::Active => "active",
            BranchStatus::Provisioning => "provisioning",
            BranchStatus::Resetting => "resetting",
        }
    }
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "oltp_branches")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub tenant_row_id: Uuid,
    pub kind: OltpBranch,

    /// Provider-side branch id — Neon's `br-…`, `LocalProvider`'s branch
    /// database name. Also the OLTP migration ledger's target for this branch
    /// (`branch:<this>`, see [`crate::branches::ledger_target`]).
    pub provider_branch_id: String,
    /// The production branch it was cut from.
    pub parent_branch_id: String,
    pub host: String,
    pub database_name: String,

    pub owner_role: String,
    /// AES-GCM-sealed owner password for THIS branch — same envelope as
    /// `oltp_tenants.owner_password_ciphertext`.
    ///
    /// `None` on a provider whose branches share the tenant's roles
    /// (`LocalProvider`): the tenant's own owner credential opens the branch,
    /// and there is nothing branch-only to seal.
    pub owner_password_ciphertext: Option<Vec<u8>>,

    pub status: BranchStatus,

    pub created_at: DateTimeWithTimeZone,
    /// Last on-demand reset. The branch's data is as old as
    /// `last_reset_at.unwrap_or(created_at)` — that is what staleness counts.
    pub last_reset_at: Option<DateTimeWithTimeZone>,
    pub updated_at: DateTimeWithTimeZone,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::tenants::Entity",
        from = "Column::TenantRowId",
        to = "super::tenants::Column::Id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    Tenants,
    #[sea_orm(has_many = "super::branch_roles::Entity")]
    BranchRoles,
}

impl Related<super::tenants::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Tenants.def()
    }
}

impl Related<super::branch_roles::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::BranchRoles.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
