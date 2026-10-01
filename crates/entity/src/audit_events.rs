use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One append-only audit event for a privileged/admin action. See
/// `internal-docs/partner-platform.md` §6.
///
/// No SeaORM relations by design: an audit event must outlive the rows it
/// references (deleting an org/user must not touch its history), so scope ids
/// and labels are denormalized here rather than joined. Rows are insert-only;
/// treat this Model as read-after-write.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "audit_events")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub created_at: DateTimeWithTimeZone,
    pub actor_user_id: Option<Uuid>,
    pub actor_email: String,
    /// `user` | `system` | `api_key` | `partner_admin`.
    pub actor_type: String,
    /// Versioned action name, e.g. `member.role.updated`, `custom_app.published`.
    pub action: String,
    /// Scope, denormalized at write time.
    pub org_id: Option<Uuid>,
    pub workspace_id: Option<Uuid>,
    pub partner_id: Option<Uuid>,
    pub target_type: Option<String>,
    pub target_id: Option<String>,
    pub target_label: Option<String>,
    pub before: Option<Json>,
    pub after: Option<Json>,
    pub ip: Option<String>,
    pub user_agent: Option<String>,
    pub request_id: Option<String>,
    /// `success` | `failure`.
    pub outcome: String,
    pub reason: Option<String>,
    pub metadata: Json,
    /// Per-org hash chain (tamper-evidence); populated by the emission helper.
    pub prev_hash: Option<String>,
    pub hash: Option<String>,
    /// DB-assigned monotonic insert sequence (`BIGSERIAL`). Strictly increasing,
    /// so the per-org chain is ordered by this rather than the app-generated
    /// `created_at` (skew-independent). Filled by the DB on insert.
    pub seq: i64,
    /// The custom-app environment the action happened in: `production` (the
    /// column default, and every row before environments), `staging` or a
    /// `dev-<handle>` slot (`m20260929_000001_custom_app_activity_environment`).
    pub environment: String,
}

impl ActiveModelBehavior for ActiveModel {}
