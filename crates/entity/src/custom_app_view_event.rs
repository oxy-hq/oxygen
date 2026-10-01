use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

/// One row per HTML serve of a custom-app bundle — the moment a user
/// "opened the app" in their browser. Recorded server-side from
/// `custom_apps_serve::serve_dispatch` via `tokio::spawn` so the
/// write is fire-and-forget on the response path.
///
/// The Activity tab in AppDetail queries this table for "who opened
/// this app and when?" — see also `super::custom_app_event` for the
/// engineer-tagged custom events fired from inside the bundle.
#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq, Serialize, Deserialize)]
#[sea_orm(table_name = "custom_app_view_event")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub app_id: Uuid,
    pub user_id: Uuid,
    /// Denormalized for the "recent visitors" table so we don't have
    /// to join `users` on every row. Kept in sync at insert time only;
    /// stale on email change (acceptable — operator can re-derive
    /// from `user_id`).
    pub user_email: String,
    /// Sticky 30-min cookie value. Multiple views with the same
    /// `session_id` collapse into one session for the active-users
    /// count.
    pub session_id: Uuid,
    pub viewed_at: DateTimeWithTimeZone,
    /// HTTP `Referer` header when present and same-host (third-party
    /// referrers are dropped at insert time so we don't leak external
    /// URLs into the DB). The HTTP header itself is canonically spelled
    /// `Referer` (one r — a famous early-spec typo); the plural in
    /// prose uses the dictionary spelling so the typos linter stays
    /// happy.
    pub referrer: Option<String>,
    /// `"browser"` (default — has User-Agent + Sec-Fetch-Site headers
    /// consistent with a real navigation) / `"sdk"` (the bundle SDK
    /// or curl-style probe) / `"unknown"`.
    pub user_agent_class: String,
    /// `"subpath"` (served from `/customer-apps/<org>/<slug>/`) or
    /// `"subdomain"` (served from
    /// `<org>--<slug>.customer-apps[-env].oxygen-hq.com`). Lets the
    /// admin see which surface their users are reaching.
    pub source: String,
    /// The viewer's role **in this app** at view time — `"admin"` /
    /// `"member"`, resolved through `oxy-authz` the same way
    /// `ctx.user.appRole` is.
    ///
    /// Snapshotted, not joined at read time: roles change, and a log that
    /// re-derives them retroactively rewrites its own history — the person who
    /// exported the data as an admin would render as whatever they are today.
    /// (Contrast `user_email` above, where staleness is fine precisely because
    /// it *is* re-derivable from `user_id`.)
    ///
    /// `NULL` means **not recorded** — a row predating the column, or a lookup
    /// that failed. View recording is best-effort and must never fail a page
    /// load, so an unresolvable role is absent rather than guessed. Do not read
    /// `NULL` as "no role".
    pub app_role: Option<String>,
    /// The viewer's role in the owning **org** at view time — `"owner"` /
    /// `"admin"` / `"member"`. Same snapshot and same `NULL` semantics as
    /// [`Self::app_role`].
    ///
    /// Both are recorded because they answer different questions and routinely
    /// disagree: an app admin need not be an org admin, and an org owner shows
    /// up as an app admin through break-glass without ever being granted the
    /// app. Collapsing them to one column would lose whichever the operator
    /// happened to need.
    pub org_role: Option<String>,
    /// The app environment that served the view (`production`, `staging`,
    /// `dev-<handle>`). The Activity tab counts production only, so a staff
    /// member opening staging is not an app user. `DEFAULT 'production'`.
    pub environment: String,
    #[sea_orm(
        belongs_to,
        from = "app_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub apps: BelongsTo<super::apps::Entity>,
    #[sea_orm(
        belongs_to,
        from = "user_id",
        to = "id",
        on_update = "NoAction",
        on_delete = "Cascade"
    )]
    #[serde(skip)]
    pub users: BelongsTo<super::users::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
