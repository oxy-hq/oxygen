//! The badge: how many unread notifications a person has, in all and per app.
//!
//! Separate from the inbox page on purpose. The page reads each org's store
//! for what its notifications say; the badge is polled, and a poll must not
//! wake a scale-to-zero tenant database every minute to learn a number Oxy's
//! own database already has.
//!
//! The per-app counts ride the same poll, so the home's app badges cost no
//! request of their own: one grouped count gives both. The two stream counts
//! ride it too, as conditional counts in the same query — **not** by grouping
//! on `kind` as well, which would multiply the rows a poll brings back by
//! kinds-per-app to learn two numbers.

use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;
use entity::notifications;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::sea_query::{Expr, SimpleExpr};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use tracing::instrument;
use uuid::Uuid;

use super::filter::FilterParams;
use super::stream::operations_kinds;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct UnreadResponse {
    pub unread: u64,
    /// The apps with something unread, each with its count. Platform notices
    /// (no app) are in `unread` only.
    pub apps: Vec<AppUnread>,
    /// The same total split by stream (`crate::stream`). `unread` still counts
    /// everything, so no client that predates this changes behaviour, and a
    /// person who runs nothing always reads `operations: 0`.
    pub streams: StreamUnread,
}

#[derive(Debug, Serialize, PartialEq, Eq, Default)]
pub struct StreamUnread {
    pub activity: u64,
    pub operations: u64,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct AppUnread {
    pub app_id: Uuid,
    pub unread: u64,
}

/// `GET /api/notifications/unread` — Oxy's database only.
#[instrument(skip_all)]
pub async fn unread(
    AuthenticatedUserExtractor(user): AuthenticatedUserExtractor,
    Query(q): Query<FilterParams>,
) -> Result<Json<UnreadResponse>, StatusCode> {
    let db = establish_connection()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    // The filter reads catalogues (Oxy's database) for a category, never an
    // org's store.
    let groups = q
        .resolve(&db)
        .await?
        .apply(notifications::Entity::find(), &user)
        .filter(notifications::Column::ReadAt.is_null())
        .select_only()
        .column(notifications::Column::AppId)
        .column_as(notifications::Column::Id.count(), "unread")
        // Two conditional counts in the same query and the same pass: a stream
        // is a condition over these two columns, so the database can answer it
        // without the response growing a row per kind.
        .column_as(operations_count(), "operations")
        .group_by(notifications::Column::AppId)
        .into_tuple::<(Option<Uuid>, i64, i64)>()
        .all(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(summarize(groups)))
}

/// `COUNT(*) FILTER (WHERE app_id IS NULL AND kind IN (<operations kinds>))`,
/// with the kinds **bound** rather than written into the SQL, built from
/// Oxygen's catalogue so a new kind needs no edit here.
///
/// Activity is the rest of the group's count, never a second expression: two
/// expressions could disagree, and one subtraction cannot.
fn operations_count() -> SimpleExpr {
    let kinds = operations_kinds();
    if kinds.is_empty() {
        // No standing-based kind exists, so nothing is Operations. Said out
        // loud rather than left to an empty `IN ()`.
        return Expr::cust("COUNT(*) FILTER (WHERE false)");
    }
    let markers = (1..=kinds.len())
        .map(|n| format!("${n}"))
        .collect::<Vec<_>>()
        .join(", ");
    Expr::cust_with_values(
        format!("COUNT(*) FILTER (WHERE app_id IS NULL AND kind IN ({markers}))"),
        kinds,
    )
}

/// The grouped counts as the response: the total, the apps by id, and the two
/// streams. Each group carries how many of its rows are Operations, so Activity
/// is the remainder and the two always add up to `unread`.
fn summarize(groups: Vec<(Option<Uuid>, i64, i64)>) -> UnreadResponse {
    let count = |n: i64| u64::try_from(n).unwrap_or(0);
    let unread: u64 = groups.iter().map(|(_, n, _)| count(*n)).sum();
    let operations: u64 = groups.iter().map(|(_, _, ops)| count(*ops)).sum();
    let mut apps: Vec<AppUnread> = groups
        .into_iter()
        .filter_map(|(app, n, _)| {
            app.map(|app_id| AppUnread {
                app_id,
                unread: count(n),
            })
        })
        .filter(|a| a.unread > 0)
        .collect();
    // Grouping order is the database's; a stable order keeps the poll's
    // response identical while nothing changed.
    apps.sort_by_key(|a| a.app_id);
    UnreadResponse {
        unread,
        apps,
        streams: StreamUnread {
            activity: unread.saturating_sub(operations),
            operations,
        },
    }
}

#[cfg(test)]
mod tests {
    use sea_orm::QueryTrait;

    use super::*;

    #[test]
    fn the_total_counts_platform_notices_and_the_apps_do_not() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let got = summarize(vec![(Some(b), 2, 0), (None, 3, 2), (Some(a), 1, 0)]);
        assert_eq!(got.unread, 6);
        assert_eq!(
            got.apps,
            [
                AppUnread {
                    app_id: a,
                    unread: 1
                },
                AppUnread {
                    app_id: b,
                    unread: 2
                },
            ]
        );
    }

    #[test]
    fn nothing_unread_is_zero_and_no_apps() {
        assert_eq!(
            summarize(vec![]),
            UnreadResponse {
                unread: 0,
                apps: vec![],
                streams: StreamUnread::default()
            }
        );
    }

    #[test]
    fn the_two_streams_add_up_to_the_total_and_apps_are_all_activity() {
        let app = Uuid::from_u128(7);
        // Two failed runs and a past-due invoice among the platform notices,
        // four app notices, all of which are Activity.
        let got = summarize(vec![(None, 5, 3), (Some(app), 4, 0)]);
        assert_eq!(got.unread, 9);
        assert_eq!(
            got.streams,
            StreamUnread {
                activity: 6,
                operations: 3
            }
        );
        assert_eq!(got.streams.activity + got.streams.operations, got.unread);
    }

    #[test]
    fn a_count_that_cannot_be_trusted_never_makes_activity_negative() {
        // The two counts come from one query, so operations can't exceed the
        // total — but a saturating subtraction is cheaper than finding out.
        let got = summarize(vec![(None, 1, 4)]);
        assert_eq!(got.streams.activity, 0);
    }

    #[test]
    fn the_operations_filter_binds_one_marker_per_kind_of_the_stream() {
        let kinds = operations_kinds();
        assert!(!kinds.is_empty(), "the count would be a constant zero");
        let sql = notifications::Entity::find()
            .select_only()
            .column_as(operations_count(), "operations")
            .build(sea_orm::DatabaseBackend::Postgres)
            .to_string();
        assert!(sql.contains("app_id IS NULL"), "{sql}");
        // Bound, never interpolated — but the values still reach the query.
        for kind in kinds {
            assert!(sql.contains(&kind), "{kind} in {sql}");
        }
    }
}
