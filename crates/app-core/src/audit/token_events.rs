//! "What happened to this API token, and what was done with it?" — one filter
//! over `audit_events` (API-tokens design §3.7).
//!
//! Two kinds of row are about a token:
//!
//! - **lifecycle** events whose target it is (`token.created`, `.extended`,
//!   `.revoked`): `target_type = 'api_token'` and `target_id = <id>`;
//! - **actions performed with it**: `metadata->>'token_id' = <id>`, stamped by
//!   [`AuditEntry::for_request`](super::AuditEntry::for_request).
//!
//! Each half has a partial index
//! (`m20261005_000001_audit_events_token_indexes`). The
//! `target_type` literal is inlined rather than bound so the planner can always
//! prove the partial index's predicate, generic plan or not.

use entity::audit_events;
use entity::prelude::AuditEvents;
use sea_orm::sea_query::{Expr, ExprTrait};
use sea_orm::{
    ColumnTrait, Condition, DatabaseConnection, DbErr, EntityTrait, QueryFilter, QueryOrder,
    QuerySelect,
};
use uuid::Uuid;

/// The `target_type` every token lifecycle event carries.
pub const TOKEN_TARGET_TYPE: &str = "api_token";

/// Rows that are about `token_id`, either way.
pub(super) fn about_token(token_id: Uuid) -> Condition {
    let id = token_id.to_string();
    Condition::any()
        .add(
            Condition::all()
                .add(Expr::cust(format!("target_type = '{TOKEN_TARGET_TYPE}'")))
                .add(audit_events::Column::TargetId.eq(id.clone())),
        )
        .add(Expr::cust("metadata->>'token_id'").eq(id))
}

/// The newest `limit` events about one token: its lifecycle and the actions
/// performed with it. Bounded by `limit` and by the audit retention window.
pub async fn events_for_token(
    db: &DatabaseConnection,
    token_id: Uuid,
    limit: u64,
) -> Result<Vec<audit_events::Model>, DbErr> {
    AuditEvents::find()
        .filter(about_token(token_id))
        .order_by_desc(audit_events::Column::Seq)
        .limit(limit)
        .all(db)
        .await
}

/// As [`events_for_token`], but only the rows in `org_id`'s chain — what an
/// org's admins may see of a token that also acts elsewhere.
pub async fn events_for_token_in_org(
    db: &DatabaseConnection,
    token_id: Uuid,
    org_id: Uuid,
    limit: u64,
) -> Result<Vec<audit_events::Model>, DbErr> {
    AuditEvents::find()
        .filter(about_token(token_id))
        .filter(audit_events::Column::OrgId.eq(org_id))
        .order_by_desc(audit_events::Column::Seq)
        .limit(limit)
        .all(db)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{DbBackend, QueryTrait};

    #[test]
    fn the_filter_matches_a_target_or_a_metadata_token_id() {
        let id = Uuid::new_v4();
        let sql = AuditEvents::find()
            .filter(about_token(id))
            .build(DbBackend::Postgres)
            .to_string();
        // Both halves in the shape their partial index is declared in.
        let target =
            format!("(target_type = 'api_token') AND \"audit_events\".\"target_id\" = '{id}'");
        let performed = format!("(metadata->>'token_id') = '{id}'");
        assert!(sql.contains(&target), "{sql}");
        assert!(sql.contains(&performed), "{sql}");
        assert!(sql.contains(&format!("({target}) OR {performed}")), "{sql}");
    }
}
