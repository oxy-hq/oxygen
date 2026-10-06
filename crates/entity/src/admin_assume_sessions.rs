use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// An explicit, time-bounded staff impersonation ("assume role") session.
///
/// The platform has always let Oxy staff act as a tenant Owner — `org_context` /
/// `workspace_context` synthesize an Owner membership for a Global Owner /
/// Global Admin who is not a real member. This row is what makes that reach
/// **opt-in, scoped, bounded, and auditable**: no live row for `(actor, org)`,
/// no synthetic membership.
///
/// `actor_user_id` / `actor_email` are always the REAL staff user — never the
/// impersonated identity, so the audit trail names who actually acted.
///
/// A session is live when `ended_at IS NULL AND expires_at > now()`.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "admin_assume_sessions")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub actor_user_id: Uuid,
    pub actor_email: String,
    /// Scope: exactly one org.
    pub org_id: Uuid,
    /// Why — required. An unexplained impersonation is a red flag.
    pub reason: String,
    pub started_at: DateTimeWithTimeZone,
    /// Hard bound; an expired row grants nothing.
    pub expires_at: DateTimeWithTimeZone,
    /// Set when explicitly ended; NULL while live.
    pub ended_at: Option<DateTimeWithTimeZone>,
    /// The new-format API token that opened the session, which is then the only
    /// credential it applies to. `None` = opened in a browser: every browser
    /// session of the actor shares it, and so does a legacy key (API-tokens
    /// design §3.2, §3.5). No foreign key — see the migration.
    pub token_id: Option<Uuid>,
    #[sea_orm(
        belongs_to,
        from = "org_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub organizations: BelongsTo<super::organizations::Entity>,
    #[sea_orm(
        belongs_to,
        from = "actor_user_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub users: BelongsTo<super::users::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
