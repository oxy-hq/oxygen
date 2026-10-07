//! Which **instance** of a sandbox name a sandbox agent token reads (sandbox
//! agent credential design §1, rows R1–R3, C3, L1 and F1).
//!
//! What a sandbox ran is kept under its environment's *name*: an invocation
//! row, the held writes it points at, a queued run, a log line. Those outlive
//! the sandbox, and the name is free again once its teardown finishes. So
//! "the rows of `dev-a`" are the rows of every sandbox that was ever called
//! `dev-a` — a colleague's, deleted last week, included. Owning the `dev-a`
//! that exists now must not open those.
//!
//! The bound is the row's own `created_at`: a token is shown only what was
//! written **at or after its sandbox came to be**. That needs no new column
//! because names cannot overlap in time — a name is taken until its row is
//! deleted, and the row is deleted last, after the sandbox's homes
//! (`custom_apps_sandboxes::teardown`) — so everything an earlier sandbox
//! under the name wrote is older than the row that replaced it.
//!
//! Secrets need no bound: the teardown deletes them before it frees the name.
//!
//! **Staging has a bound too, for a token granted it.** Staging is not a
//! sandbox and no token created it: its rows are every staff member's, from
//! before the token existed. A token minted with `staging` reads what staging
//! wrote **since it was granted** — the `created_at` of its `app_staging`
//! grant for the app — and nothing older. The grant is read from its row on
//! every ask, so a token with no such grant, or a revoked one, has no start
//! and reads nothing of staging.
//!
//! Asked for a sandbox agent token only. Every other caller reads by name, as
//! before, with no extra read.

use chrono::{DateTime, Utc};
use entity::app_environments;
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter};
use uuid::Uuid;

/// When the sandbox `environment` of `app_id` that `token` created came to
/// be. `None` when the row under the name is another creator's, or there is
/// none: the token then reads nothing of that name.
///
/// Read from the same row the ownership is decided on, so a sandbox created
/// again by someone else between the two cannot lend the token its start.
///
/// For **staging** it is when the token was granted the app's staging, read
/// from that grant's row; `None` for a token that holds none. Production has
/// no start for any token.
pub(crate) async fn own_since<C: ConnectionTrait>(
    db: &C,
    app_id: Uuid,
    environment: &AppEnvironment,
    token: Uuid,
) -> Result<Option<DateTime<Utc>>, DbErr> {
    match environment {
        AppEnvironment::Production => return Ok(None),
        AppEnvironment::Staging => {
            return oxy_auth::token::sandbox::staging_since(db, token, app_id).await;
        }
        AppEnvironment::Dev { .. } => {}
    }
    let row = app_environments::Entity::find()
        .filter(app_environments::Column::AppId.eq(app_id))
        .filter(app_environments::Column::Name.eq(environment.name()))
        .one(db)
        .await?;
    Ok(row.as_ref().and_then(|row| start_of(row, token)))
}

/// `row`'s start, when `token` created it.
fn start_of(row: &app_environments::Model, token: Uuid) -> Option<DateTime<Utc>> {
    (row.created_by_token_id == Some(token)).then(|| row.created_at.with_timezone(&Utc))
}

/// Whether something written at `written_at` belongs to the instance that
/// started at `since`: at or after it.
pub(crate) fn is_of_instance(written_at: DateTime<Utc>, since: DateTime<Utc>) -> bool {
    written_at >= since
}

/// [`is_of_instance`] for a log line's timestamp as the log store serves it
/// (ISO-8601 UTC). A timestamp that does not parse belongs to no instance.
pub(crate) fn line_is_of_instance(timestamp: &str, since: DateTime<Utc>) -> bool {
    DateTime::parse_from_rfc3339(timestamp)
        .is_ok_and(|at| is_of_instance(at.with_timezone(&Utc), since))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    const TOKEN: Uuid = Uuid::from_u128(0x70);

    fn at(hour: u32, second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 6, hour, 0, second).unwrap()
    }

    fn row(created_by_token_id: Option<Uuid>) -> app_environments::Model {
        app_environments::Model {
            app_id: Uuid::from_u128(1),
            name: "dev-a".into(),
            kind: "dev".into(),
            owner_user_id: None,
            build_id: None,
            updated_by: None,
            updated_at: at(9, 0).fixed_offset(),
            created_at: at(9, 0).fixed_offset(),
            deleting_at: None,
            oltp_schema: None,
            created_by_token_id,
        }
    }

    /// The start is the token's to use only on the row it created: a
    /// colleague's sandbox under the name, and another token's, lend none.
    #[test]
    fn only_the_row_the_token_created_has_a_start() {
        assert_eq!(start_of(&row(Some(TOKEN)), TOKEN), Some(at(9, 0)));
        assert_eq!(start_of(&row(None), TOKEN), None);
        assert_eq!(start_of(&row(Some(Uuid::from_u128(0x71))), TOKEN), None);
    }

    /// Everything from the sandbox's first moment on is its own; anything
    /// older was an earlier sandbox's under the same name.
    #[test]
    fn a_row_belongs_to_the_instance_from_its_start_on() {
        let since = at(9, 0);
        assert!(!is_of_instance(at(8, 59), since), "an earlier sandbox's");
        assert!(is_of_instance(since, since), "its first moment");
        assert!(is_of_instance(at(9, 1), since));
    }

    /// A log line is judged by the timestamp the store serves; one that does
    /// not parse is not shown.
    #[test]
    fn a_log_line_is_judged_by_its_served_timestamp() {
        let since = at(9, 0);
        assert!(line_is_of_instance("2026-10-06T09:00:01.250000Z", since));
        assert!(line_is_of_instance("2026-10-06T09:00:00.000000Z", since));
        assert!(!line_is_of_instance("2026-10-06T08:59:59.999999Z", since));
        for unreadable in ["", "yesterday", "2026-10-06 09:00:01"] {
            assert!(!line_is_of_instance(unreadable, since), "{unreadable:?}");
        }
    }
}
