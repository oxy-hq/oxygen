//! Standing tokens: the personal tokens that carry their owner's standing —
//! `platform` (Oxy staff) or `partner` (a distributor reaching its client
//! orgs). The reads behind `/api/admin/standing-tokens` (API-tokens design,
//! "Standing tokens, for staff").
//!
//! A standing is the strongest thing a credential can carry, and an
//! all-access token carrying one is seen by nobody but its owner: it holds no
//! grant in any org, so no org's inventory lists it. These reads are how the
//! staff who answer for who holds staff access see them.
//!
//! Like [`super::sandbox`]'s staff reads, this is database primitives only.
//! *Who* may ask is the handler's, as is the audit row of a revoke.
//!
//! **Nothing here narrows by org, on purpose.** A standing token is a
//! credential for the whole deployment, not for the orgs it happens to hold a
//! grant in, so there is no meaningful subset of these rows for a platform
//! grant bounded to some orgs. The handler admits an unbounded grant only and
//! then reads every row that still works, and the newest ended ones.

use chrono::Utc;
use entity::api_tokens;
use entity::prelude::ApiTokens;
use oxy_shared::errors::OxyError;
use sea_orm::prelude::DateTimeWithTimeZone;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    Select,
};
use uuid::Uuid;

use super::credential::StoredKind;

fn db_err(what: &'static str) -> impl FnOnce(sea_orm::DbErr) -> OxyError {
    move |e| OxyError::DBError(format!("{what}: {e}"))
}

/// Whether a row is a standing token: a personal token with `platform` or
/// `partner` set.
///
/// Never a **legacy key** — a row that mirrors `api_keys` stores both
/// standings always, and is managed through its own routes — nor a sandbox
/// agent token (its own staff list), nor a service-account or CI token (an
/// account holds no standing).
pub fn carries_standing(row: &api_tokens::Model) -> bool {
    row.kind == StoredKind::Personal.as_str()
        && row.legacy_api_key_id.is_none()
        && (row.platform || row.partner)
}

/// The query behind [`list`]: [`carries_standing`], said in SQL.
fn select(limit: u64) -> Select<ApiTokens> {
    ApiTokens::find()
        .filter(api_tokens::Column::Kind.eq(StoredKind::Personal.as_str()))
        .filter(api_tokens::Column::LegacyApiKeyId.is_null())
        .filter(
            Condition::any()
                .add(api_tokens::Column::Platform.eq(true))
                .add(api_tokens::Column::Partner.eq(true)),
        )
        .order_by_desc(api_tokens::Column::CreatedAt)
        .order_by_desc(api_tokens::Column::Id)
        .limit(limit)
}

/// A standing token that still works at `now`: not revoked, and not past its
/// expiry. Its negation is "ended" with no third case — neither half of the
/// test is ever unknown, because a missing expiry is asked for by name.
fn works_at(now: DateTimeWithTimeZone) -> Condition {
    Condition::all()
        .add(api_tokens::Column::RevokedAt.is_null())
        .add(
            Condition::any()
                .add(api_tokens::Column::ExpiresAt.is_null())
                .add(api_tokens::Column::ExpiresAt.gt(now)),
        )
}

/// One listing from its two halves, newest first.
fn newest_first(
    mut working: Vec<api_tokens::Model>,
    ended: Vec<api_tokens::Model>,
) -> Vec<api_tokens::Model> {
    working.extend(ended);
    working.sort_by(|a, b| (b.created_at, b.id).cmp(&(a.created_at, a.id)));
    working
}

/// The staff view, newest first, for a caller the handler has already found
/// unbounded: **every standing token that still works**, then the newest
/// ended ones, `limit` rows in all.
///
/// Not simply the newest `limit` rows. Each `oxyc login` retires its owner's
/// previous token, so ended rows pile up far faster than working ones, and in
/// time they would push a token that still works off the end of the list —
/// the one row the list exists to show. The working ones are read first and
/// the ended ones fill what room is left.
pub async fn list<C: ConnectionTrait>(
    db: &C,
    limit: u64,
) -> Result<Vec<api_tokens::Model>, OxyError> {
    let now = Utc::now().fixed_offset();
    let working = select(limit)
        .filter(works_at(now))
        .all(db)
        .await
        .map_err(db_err("list working standing tokens"))?;
    let room = limit.saturating_sub(working.len() as u64);
    if room == 0 {
        return Ok(working);
    }
    let ended = select(room)
        .filter(works_at(now).not())
        .all(db)
        .await
        .map_err(db_err("list ended standing tokens"))?;
    Ok(newest_first(working, ended))
}

/// One standing token by id, whoever owns it. `None` for any other row, so
/// the staff routes cannot be pointed at an ordinary personal token, a legacy
/// key, or a token of another kind.
pub async fn find<C: ConnectionTrait>(
    db: &C,
    token_id: Uuid,
) -> Result<Option<api_tokens::Model>, OxyError> {
    Ok(ApiTokens::find_by_id(token_id)
        .one(db)
        .await
        .map_err(db_err("standing token lookup"))?
        .filter(carries_standing))
}

#[cfg(test)]
#[path = "standing_tests.rs"]
mod tests;
