use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One named environment of a custom app: `production`, `staging`, or a
/// `dev-<handle>` sandbox. `build_id` names the build that environment serves.
///
/// This row is the only store of which build an environment serves; the
/// production and staging rows are written by
/// `custom_apps_environments::record_move`, in the transaction that holds the
/// app's row. A sandbox's row is created and deleted by
/// `custom_apps_sandboxes`; `record_move` only moves its pointer.
/// Naming rules: `oxy_app_core::custom_app_environment`.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "app_environments")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub app_id: Uuid,
    #[sea_orm(primary_key, auto_increment = false)]
    pub name: String,
    /// `production` | `staging` | `dev` (DB CHECK).
    pub kind: String,
    /// Set for `dev` slots only (DB CHECK).
    pub owner_user_id: Option<Uuid>,
    pub build_id: Option<Uuid>,
    pub updated_by: Option<Uuid>,
    pub updated_at: DateTimeWithTimeZone,
    pub created_at: DateTimeWithTimeZone,
    /// Set while a sandbox is being torn down: it serves nothing and its name
    /// stays taken until the teardown task removes the row. Always `None` for
    /// `production` and `staging`.
    pub deleting_at: Option<DateTimeWithTimeZone>,
    /// A sandbox's own schema on the org's OLTP staging branch, as the
    /// platform last left it: its name, `seeding` / `ready` / `failed`, and
    /// the branch cut it was seeded on (`custom_apps_sandboxes::oltp_state`).
    /// `None` until a publish to the sandbox queues it, and always for
    /// `production` and `staging`.
    #[sea_orm(column_type = "JsonBinary", nullable)]
    pub oltp_schema: Option<Json>,
    /// The sandbox agent token (`api_tokens.id`, kind `sandbox_agent`) that
    /// created this sandbox: what "its own sandbox" means for that token.
    /// `None` for every environment a person created, and always for
    /// `production` and `staging`. No foreign key: an id whose token row is
    /// gone matches no credential, so the sandbox has no token owner.
    pub created_by_token_id: Option<Uuid>,
    /// Whether this environment fires its functions' `schedule`s. Staff turn
    /// it on per environment (`custom_apps_env_schedules`); `false` by
    /// default, and never read for `production`, whose schedules always run.
    pub schedules_enabled: bool,
    /// The environment this one was cut from. `None` means the default parent
    /// for the kind (a sandbox's is `staging`, staging's is `production`),
    /// and always for `production`, which has none.
    pub parent_name: Option<String>,
    /// The point on the parent this environment was cut at. `None` means the
    /// default for the kind: when the environment was created.
    pub fork_at: Option<DateTimeWithTimeZone>,
    /// Per-plane mode choices for this environment, keyed by plane. Never
    /// null; `{}` means every plane uses the default for the kind.
    #[sea_orm(column_type = "JsonBinary")]
    pub planes: Json,
    /// The compiled revision this environment's semantic plane reads when its
    /// build pins nothing of its own. Served while it is a `ready` revision of
    /// the app's workspace, even after `semantic_git_sha` has moved on. `None`
    /// means no environment pin, and always for `production`. No foreign key:
    /// an id with no ready revision behind it reads as "not pinned".
    pub semantic_revision_id: Option<Uuid>,
    /// The commit the environment's semantic branch points at. With no ready
    /// revision pinned, a set value means the plane's home is not ready, and
    /// the environment is refused (`503 semantic_branch_not_ready`) rather
    /// than routed to another model.
    pub semantic_git_sha: Option<String>,
}

impl ActiveModelBehavior for ActiveModel {}
