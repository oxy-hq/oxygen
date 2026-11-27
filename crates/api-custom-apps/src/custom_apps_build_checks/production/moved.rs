//! The move a production attempt checks, read from production's pointer
//! history (`app_environment_events`): who made it, how, and what production
//! served before it.
//!
//! Read when the attempt's check has failed, not when it was queued: the
//! history is the one record of production's moves, and what an automatic
//! rollback may do depends on what production serves **now**.

use chrono::{DateTime, FixedOffset};
use entity::{app_builds, app_environment_events as events};
use oxy_app_core::custom_app_environment::AppEnvironment;
use sea_orm::{
    ColumnTrait, ConnectionTrait, DbErr, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
};
use serde::Serialize;
use uuid::Uuid;

use crate::custom_apps_environments::EnvAction;

/// How far back production's history is read: far more moves than separate a
/// failed check from the move it checked.
const RECENT: u64 = 200;

/// A build, by both of its ids.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BuildName {
    /// `app_builds.id`.
    pub id: Uuid,
    /// The publish id.
    pub build_id: String,
}

/// The newest move of production to one build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Move {
    /// `promote`, `rollback` or `publish`, as the event recorded it.
    pub action: String,
    /// Who moved production; `None` for a move no user made.
    pub by: Option<Uuid>,
    pub at: DateTime<FixedOffset>,
    /// The build production served most recently before it; `None` when it
    /// served no other build.
    pub previous: Option<Uuid>,
    /// Whether that move is still production's newest: nothing has moved
    /// production since.
    pub newest: bool,
}

impl Move {
    /// The move was a rollback, a person's or the platform's.
    pub fn was_rollback(&self) -> bool {
        self.action == EnvAction::Rollback.as_str()
    }
}

/// One event, as the history is read here: `(build, action, actor, at)`.
pub type Event = (Option<Uuid>, String, Option<Uuid>, DateTime<FixedOffset>);

/// The build production was rolled back to by the platform, straight from
/// `build`: production's newest move is a rollback no user made, and the move
/// before it was to `build`. `None` otherwise. Every rollback a person makes
/// names them, so this is the automatic one, and it is how a task that died
/// between its rollback and writing it down finds out what it did.
pub fn platform_rollback_from(history: &[Event], build: Uuid) -> Option<Uuid> {
    let (to, action, by, _) = history.first()?;
    let from = history.get(1)?.0;
    let automatic = action == EnvAction::Rollback.as_str() && by.is_none();
    to.filter(|_| automatic && from == Some(build))
}

/// The newest move to `build` in `history` (newest first).
pub fn move_in(history: &[Event], build: Uuid) -> Option<Move> {
    let at = history.iter().position(|event| event.0 == Some(build))?;
    let (_, action, by, when) = &history[at];
    let previous = history[at + 1..]
        .iter()
        .find_map(|event| event.0.filter(|other| *other != build));
    Some(Move {
        action: action.clone(),
        by: *by,
        at: *when,
        previous,
        newest: at == 0,
    })
}

/// `app_id`'s recent production moves, newest first.
pub async fn history<C: ConnectionTrait>(conn: &C, app_id: Uuid) -> Result<Vec<Event>, DbErr> {
    events::Entity::find()
        .filter(events::Column::AppId.eq(app_id))
        .filter(events::Column::Environment.eq(AppEnvironment::Production.name()))
        .select_only()
        .column(events::Column::BuildId)
        .column(events::Column::Action)
        .column(events::Column::Actor)
        .column(events::Column::At)
        .order_by_desc(events::Column::At)
        .limit(RECENT)
        .into_tuple()
        .all(conn)
        .await
}

/// `build_pk` by both of its ids; `None` when it was pruned.
pub async fn named<C: ConnectionTrait>(
    conn: &C,
    build_pk: Option<Uuid>,
) -> Result<Option<BuildName>, DbErr> {
    let Some(build_pk) = build_pk else {
        return Ok(None);
    };
    Ok(app_builds::Entity::find_by_id(build_pk)
        .one(conn)
        .await?
        .map(|build| BuildName {
            id: build.id,
            build_id: build.build_id,
        }))
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};

    use super::*;

    const A: Uuid = Uuid::from_u128(0xa);
    const B: Uuid = Uuid::from_u128(0xb);
    const C: Uuid = Uuid::from_u128(0xc);
    const ANA: Uuid = Uuid::from_u128(1);

    /// Events newest first, one minute apart.
    fn history(events: &[(Option<Uuid>, &str, Option<Uuid>)]) -> Vec<Event> {
        let newest = Utc.with_ymd_and_hms(2026, 11, 22, 12, 0, 0).unwrap();
        events
            .iter()
            .enumerate()
            .map(|(i, (build, action, by))| {
                let at = (newest - Duration::minutes(i as i64)).fixed_offset();
                (*build, action.to_string(), *by, at)
            })
            .collect()
    }

    #[test]
    fn the_move_names_who_made_it_and_the_build_production_served_before() {
        let h = history(&[(Some(B), "promote", Some(ANA)), (Some(A), "promote", None)]);
        let moved = move_in(&h, B).expect("the move to B");
        assert_eq!((moved.by, moved.previous), (Some(ANA), Some(A)));
        assert!(moved.newest && !moved.was_rollback());
        // The first build production ever served replaced nothing.
        let first = move_in(&h, A).expect("the move to A");
        assert_eq!(
            (first.by, first.previous, first.newest),
            (None, None, false)
        );
        assert_eq!(move_in(&h, C), None);
    }

    /// An unpublish, a pruned build and a re-stamp of the same build are not
    /// "the build before": the newest other build is.
    #[test]
    fn the_build_before_is_the_newest_other_build_production_served() {
        let h = history(&[
            (Some(B), "promote", Some(ANA)),
            (Some(B), "promote", Some(ANA)),
            (None, "unpublish", Some(ANA)),
            (None, "promote", None),
            (Some(A), "promote", None),
        ]);
        let moved = move_in(&h, B).unwrap();
        assert_eq!(moved.previous, Some(A));
        assert!(moved.newest);
    }

    #[test]
    fn only_a_rollback_no_user_made_straight_from_the_build_is_the_platforms() {
        let automatic = history(&[(Some(A), "rollback", None), (Some(B), "promote", Some(ANA))]);
        assert_eq!(platform_rollback_from(&automatic, B), Some(A));
        assert_eq!(platform_rollback_from(&automatic, C), None, "not from C");
        let a_persons = history(&[
            (Some(A), "rollback", Some(ANA)),
            (Some(B), "promote", Some(ANA)),
        ]);
        assert_eq!(platform_rollback_from(&a_persons, B), None);
        let a_promote = history(&[(Some(A), "promote", None), (Some(B), "promote", None)]);
        assert_eq!(platform_rollback_from(&a_promote, B), None);
        assert_eq!(platform_rollback_from(&[], B), None);
    }

    #[test]
    fn a_move_production_has_left_behind_is_not_the_newest_and_a_rollback_says_so() {
        let h = history(&[
            (Some(A), "rollback", None),
            (Some(B), "promote", Some(ANA)),
            (Some(A), "promote", Some(ANA)),
        ]);
        let to_b = move_in(&h, B).unwrap();
        assert!(!to_b.newest, "production has moved since");
        let back = move_in(&h, A).unwrap();
        assert!(back.newest && back.was_rollback());
        assert_eq!((back.by, back.previous), (None, Some(B)));
    }
}
