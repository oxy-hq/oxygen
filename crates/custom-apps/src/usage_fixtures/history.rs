//! A fixed usage history for one app, to put in both stores.

use chrono::{DateTime, Duration, SubsecRound, Utc};
use entity::{custom_app_event, custom_app_view_event};
use sea_orm::ActiveValue::Set;
use uuid::Uuid;

/// The people and the app a [`history`] is about. The caller seeds the app and
/// the four users (the usage tables reference both).
#[derive(Clone, Debug)]
pub struct Cast {
    pub app_id: Uuid,
    pub users: [(Uuid, String); 4],
}

impl Cast {
    pub fn new(app_id: Uuid) -> Self {
        let user = |n: u32| (Uuid::new_v4(), format!("user{n}-{app_id}@example.com"));
        Self {
            app_id,
            users: [user(1), user(2), user(3), user(4)],
        }
    }
}

/// Rows to insert into `custom_app_view_event` and `custom_app_event`.
pub struct History {
    pub views: Vec<custom_app_view_event::ActiveModel>,
    pub events: Vec<custom_app_event::ActiveModel>,
}

impl History {
    /// Counts of production rows, for a test that wants to say what the
    /// fixture holds: `(views, events)`.
    pub fn production_rows(&self) -> (usize, usize) {
        let views = self
            .views
            .iter()
            .filter(|v| v.environment.as_ref() == "production")
            .count();
        let events = self
            .events
            .iter()
            .filter(|e| e.environment.as_ref() == "production")
            .count();
        (views, events)
    }
}

/// A usage history for `cast.app_id`, placed by hours before `anchor`. Hours
/// under 168 fall in the week ending at `anchor`, 168 to 336 in the week
/// before. Every instant carries microseconds (never a whole second), because
/// that is the precision both stores keep.
///
/// What it holds, for the readers to get right:
/// * production views by four people over six sessions, two of them repeat
///   visitors, one seen only in the week before;
/// * a visitor whose latest view recorded no role (the latest *recorded* one
///   must win), and one whose roles were never recorded;
/// * a staging view and a sandbox view, which no reader counts;
/// * production events under three names, one of them the reserved `oxy-error`
///   in two sessions this week and one the week before, and a staging event.
pub fn history(cast: &Cast, anchor: DateTime<Utc>) -> History {
    // Callers pass `Utc::now()`, which reads nanoseconds on Linux; every offset
    // below is whole microseconds, so trimming the anchor is what keeps each
    // instant at the precision the stores keep.
    let anchor = anchor.trunc_subsecs(6);
    let [(u1, e1), (u2, e2), (u3, e3), (u4, e4)] = cast.users.clone();
    let sessions: Vec<Uuid> = (0..8).map(|_| Uuid::new_v4()).collect();
    let mut tick = 0i64;
    let mut when = |hours: i64| {
        tick += 1;
        (anchor - Duration::hours(hours) + Duration::microseconds(100_000 + tick * 7_919))
            .fixed_offset()
    };
    let view = |user: (Uuid, &str),
                session: usize,
                at: DateTime<chrono::FixedOffset>,
                app_role: Option<&str>,
                org_role: Option<&str>,
                environment: &str| custom_app_view_event::ActiveModel {
        id: Set(Uuid::new_v4()),
        app_id: Set(cast.app_id),
        user_id: Set(user.0),
        user_email: Set(user.1.to_string()),
        session_id: Set(sessions[session]),
        viewed_at: Set(at),
        referrer: Set((session % 2 == 0).then(|| "/orders".to_string())),
        user_agent_class: Set("browser".into()),
        source: Set("subpath".into()),
        app_role: Set(app_role.map(str::to_string)),
        org_role: Set(org_role.map(str::to_string)),
        environment: Set(environment.into()),
    };
    let event = |user: (Uuid, &str),
                 session: usize,
                 name: &str,
                 payload: serde_json::Value,
                 at: DateTime<chrono::FixedOffset>,
                 environment: &str| custom_app_event::ActiveModel {
        id: Set(Uuid::new_v4()),
        app_id: Set(cast.app_id),
        user_id: Set(user.0),
        user_email: Set(user.1.to_string()),
        session_id: Set(sessions[session]),
        event_name: Set(name.to_string()),
        payload: Set(payload),
        occurred_at: Set(at),
        environment: Set(environment.into()),
    };
    let (a1, a2, a3, a4) = (
        (u1, e1.as_str()),
        (u2, e2.as_str()),
        (u3, e3.as_str()),
        (u4, e4.as_str()),
    );
    let views = vec![
        // This week.
        view(a1, 0, when(2), Some("admin"), Some("owner"), "production"),
        view(a1, 0, when(3), Some("admin"), Some("owner"), "production"),
        view(a1, 1, when(100), Some("member"), None, "production"),
        // The latest view recorded no role; the earlier ones did.
        view(a2, 2, when(5), None, None, "production"),
        view(a2, 2, when(6), Some("member"), Some("member"), "production"),
        view(a2, 2, when(30), Some("admin"), Some("admin"), "production"),
        view(a3, 3, when(150), None, None, "production"),
        // Never counted: another environment's.
        view(a1, 0, when(1), Some("admin"), Some("owner"), "staging"),
        view(a4, 6, when(4), Some("admin"), Some("owner"), "dev-ada"),
        // The week before.
        view(a1, 4, when(200), Some("admin"), Some("owner"), "production"),
        view(
            a2,
            5,
            when(250),
            Some("member"),
            Some("member"),
            "production",
        ),
        view(a4, 7, when(300), Some("member"), None, "production"),
        view(a4, 7, when(301), Some("member"), None, "production"),
    ];
    let events = vec![
        event(
            a1,
            0,
            "export-clicked",
            serde_json::json!({ "format": "csv" }),
            when(4),
            "production",
        ),
        event(
            a2,
            2,
            "export-clicked",
            serde_json::json!({ "format": "xlsx", "rows": 12 }),
            when(40),
            "production",
        ),
        event(
            a1,
            1,
            "export-clicked",
            serde_json::json!({}),
            when(90),
            "production",
        ),
        event(
            a3,
            3,
            "filter-applied",
            serde_json::json!({ "by": "region" }),
            when(60),
            "production",
        ),
        // Page errors the beacon wrote: counted by session, payload never read.
        event(
            a1,
            0,
            "oxy-error",
            serde_json::json!({ "message": "boom" }),
            when(8),
            "production",
        ),
        event(
            a2,
            2,
            "oxy-error",
            serde_json::json!({ "message": "boom" }),
            when(9),
            "production",
        ),
        event(
            a2,
            2,
            "oxy-error",
            serde_json::json!({ "message": "again" }),
            when(10),
            "production",
        ),
        // The week before.
        event(
            a1,
            4,
            "oxy-error",
            serde_json::json!({ "message": "old" }),
            when(210),
            "production",
        ),
        event(
            a1,
            4,
            "export-clicked",
            serde_json::json!({}),
            when(220),
            "production",
        ),
        // Never counted.
        event(
            a1,
            0,
            "export-clicked",
            serde_json::json!({}),
            when(2),
            "staging",
        ),
    ];
    History { views, events }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Linux clock reads nanoseconds and the stores keep microseconds, so an
    /// anchor taken from `Utc::now()` must not leak its sub-microsecond part
    /// into the history, or a test comparing a read back against the fixture
    /// fails only in CI.
    #[test]
    fn every_instant_is_whole_microseconds_even_from_a_nanosecond_anchor() {
        let anchor = DateTime::parse_from_rfc3339("2026-10-26T06:58:05.205051100Z")
            .unwrap()
            .with_timezone(&Utc);
        let history = history(&Cast::new(Uuid::new_v4()), anchor);
        for view in &history.views {
            assert_eq!(view.viewed_at.as_ref().timestamp_subsec_nanos() % 1_000, 0);
        }
        for event in &history.events {
            assert_eq!(
                event.occurred_at.as_ref().timestamp_subsec_nanos() % 1_000,
                0
            );
        }
    }
}
