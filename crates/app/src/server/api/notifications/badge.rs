//! The badge: how many unread notifications a person has, in all and per app.
//!
//! Separate from the inbox page on purpose. The page reads each org's store
//! for what its notifications say; the badge is polled, and a poll must not
//! wake a scale-to-zero tenant database every minute to learn a number Oxy's
//! own database already has.
//!
//! The per-app counts ride the same poll, so the home's app badges cost no
//! request of their own: one grouped count gives both.

use axum::Json;
use axum::extract::Query;
use axum::http::StatusCode;
use entity::notifications;
use oxy::database::client::establish_connection;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QuerySelect};
use serde::Serialize;
use tracing::instrument;
use uuid::Uuid;

use super::filter::FilterParams;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct UnreadResponse {
    pub unread: u64,
    /// The apps with something unread, each with its count. Platform notices
    /// (no app) are in `unread` only.
    pub apps: Vec<AppUnread>,
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
        .group_by(notifications::Column::AppId)
        .into_tuple::<(Option<Uuid>, i64)>()
        .all(&db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(summarize(groups)))
}

/// The grouped counts as the response: the total, and the apps by id.
fn summarize(groups: Vec<(Option<Uuid>, i64)>) -> UnreadResponse {
    let count = |n: i64| u64::try_from(n).unwrap_or(0);
    let unread = groups.iter().map(|(_, n)| count(*n)).sum();
    let mut apps: Vec<AppUnread> = groups
        .into_iter()
        .filter_map(|(app, n)| {
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
    UnreadResponse { unread, apps }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_total_counts_platform_notices_and_the_apps_do_not() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let got = summarize(vec![(Some(b), 2), (None, 3), (Some(a), 1)]);
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
                apps: vec![]
            }
        );
    }
}
