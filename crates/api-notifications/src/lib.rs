//! Notifications — telling a person something, and the inbox they read it in.
//!
//! # Who, not which channel
//!
//! A sender names **people** — a user, a role at a location, a team — and the
//! platform decides how each one is reached. That is the whole reason this is a
//! platform service rather than `ctx.email.send` plus `ctx.fetch`: Oxy owns the
//! identity (members, positions, frontline crew with no mailbox) and keeps the
//! addresses away from app code. Custom apps reach it through `ctx.notify`
//! ([`send::send`]); the product itself through [`platform::notify`], whose
//! notices store a kind and a subject and are rendered from Oxy's records.
//!
//! # Two halves, deliberately not one
//!
//! The **inbox** is a durable row a person opens and reads. It needs no
//! vendor, no credentials and no network, and it is written before anything
//! else is attempted.
//!
//! **Delivery** — Web Push, email, a Slack DM — is a best-effort attempt to
//! draw attention to that row, queued as a `notification_deliver` task
//! ([`task`]) and recorded per recipient and channel ([`delivery`]). A
//! notification that existed only as a push would be gone when the send
//! failed, and "we told them" would become unfalsifiable at exactly the moment
//! somebody asks whether the store was warned.
//!
//! # Where each one sits
//!
//! Every notification is a leaf of a tree — source (Oxygen, or one app) →
//! category path → kind — declared by the app's live build or, for Oxygen,
//! in code ([`catalog`]). It gives the inbox its labels and levels, gives a
//! kind its buttons ([`actions`]), and is the shape a phone's categories
//! and channels need ([`deliver::PushSignal`]).
//!
//! # What a person chose
//!
//! Pause, quiet hours, per-node channels and floors, and per-device settings
//! and OS state ([`settings`], [`devices`]), turned into "does this go out"
//! by one function, `settings::decide::decide`, which delivery records.
//!
//! # Where the data lives
//!
//! The inbox *signal* — who, which org, read or not — is Oxy's, in Oxy's
//! database. What the notification *says* is the org's, in the org's OLTP
//! store (`oxy_oltp::inbox`). Design and the rest of the reasoning:
//! `internal-docs/custom-app-notifications.md`.
//!
//! # Push transports
//!
//! Web Push ([`web_push`]), APNs ([`apns`]) and FCM ([`fcm`]) sit behind the
//! [`deliver::Push`] seam, each configured on its own, and [`push_routing`]
//! hands each its platform's device rows. None is handed what a notification
//! says — only ids and config ([`deliver::PushSignal`]): its inbox row, its
//! kind and that kind's label, category and level from the catalogue, and its
//! thread's hash, and who sent it. [`apns`] and [`fcm`] map those to their
//! payloads; [`web_push_payload`] to Web Push's, encrypted to the device.
//!
//! # Not here
//!
//! SMS needs phone numbers the platform does not hold and carrier registration
//! no code can do.
//!
//! # The crate
//!
//! A sibling surface crate (`internal-docs/domain-boundaries.md`, S1–S8), moved
//! out of `oxy-app`'s `server::api::notifications` with its paths, roles and
//! guards unchanged. It reaches `oxy-app` only through `oxy_app::surface`, and
//! `oxy-app` never names it; `oxy-server` mounts it:
//!
//! - [`routes`] rides `SurfaceSeams::api`, merged at the `/api` root behind the
//!   standard auth stack, exactly where `build_global_routes` had it. No org
//!   gate: every route is self-scoped (`user_id = me`), and a frontline worker
//!   holds no org membership.
//! - [`tasks`] rides `SurfaceSeams::tasks`: the delivery executor and the
//!   quiet-hours summary, kinds `oxy-app` refuses to start without.
//! - [`seam`] rides `SurfaceSeams::notifications`: what `oxy serve` and
//!   `oxy worker` run at boot (the inline transport, the run observers, push,
//!   the settings reconcile).
//! - [`live_kinds`] is the port the staff console's app test reads an app's
//!   kinds through, layered by `oxy-server` onto `oxy-api-custom-apps`' routes.
//!
//! What every *source* of a notice needs — the app send, the platform notice
//! write, the kinds, who — is `oxy_app_core::notifications`, because the
//! sources are spread over contexts that may not reach this crate.

// Laying out the larger handler futures (`catalog::handler`, `actions::run`)
// can pass rustc's default query depth of 128 at opt-level 3, the `--release`
// build that produces the image and that PR CI never runs. Raised before it
// overflows there, as every surface crate does.
#![recursion_limit = "256"]

pub mod action_call;
pub mod actions;
pub mod apns;
pub mod badge;
pub mod catalog;
pub mod channels;
pub mod compose;
pub mod deliver;
pub mod delivery;
pub mod devices;
pub mod fcm;
pub mod filter;
pub mod handlers;
pub mod inbox;
pub mod inbox_view;
pub mod input;
pub mod links;
mod native_push;
pub mod platform;
pub mod preferences;
pub mod push_routing;
pub mod recipients;
pub mod scope_mutes;
pub mod send;
pub mod settings;
#[cfg(debug_assertions)]
mod simulator_push;
pub mod slack_dm;
pub mod task;
pub mod test_notice;
pub mod web_push;
pub mod web_push_payload;

use std::sync::Arc;

use axum::Router;
use axum::routing::{get, patch, post};
use oxy_app::surface::seams::{NotificationsSeam, TaskSeam};
use oxy_app_core::AppState;
use oxy_app_core::notifications::catalog::live_kinds::{KindSummary, LiveKinds};
use oxy_shared::fleet_role::{RouteRole, RouteRoleDecl};

/// The notification routes, relative to the `/api` root — moved out of
/// `oxy-app`'s `build_global_routes` with their paths unchanged.
///
/// Self-scoped — the filter `user_id = me` IS the authorization, and there is
/// no org gate on purpose: a frontline worker holds no org membership by design
/// and is exactly who an overdue-work notification is for.
///
/// Every route is Postgres (and, for an item's words, the org's store) and
/// nothing on node-local disk, so every one is FleetOk ([`route_roles`]). A
/// badge that stops updating during a deploy is a badge nobody trusts
/// afterwards.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/notifications", get(inbox::list))
        // The badge: Oxy's database only, never an org's store.
        .route("/notifications/unread", get(badge::unread))
        // Sources → categories → kinds, with my mutes and unread counts. The
        // catalogues are config in Postgres (a build's manifest row, or code).
        .route("/notifications/catalog", get(catalog::handler::catalog))
        // One item, for a phone a push named it to: what its notification
        // service extension fetches. Postgres and the org's store, like the
        // inbox it is one entry of.
        .route("/notifications/{id}", get(inbox::one))
        // A notification's button. A function action runs through the app's
        // own function route, which places the invocation itself
        // (`invocation_placement`): any replica takes this.
        .route(
            "/notifications/{id}/actions/{action_id}",
            post(actions::run),
        )
        .route(
            "/notifications/preferences",
            get(preferences::get_preferences).put(preferences::set_preference),
        )
        .route("/notifications/read-all", post(handlers::mark_all_read))
        // Pause, quiet hours, every node's channels and floor, devices: one
        // document, `ETag`/`If-Match` versioned. Postgres only.
        .route(
            "/notifications/settings",
            get(settings::handlers::get_settings).patch(settings::handlers::patch_settings),
        )
        .route(
            "/notifications/devices",
            get(devices::list)
                .post(devices::register::register_device)
                .delete(devices::register::unregister_device),
        )
        // One device's settings and the OS state its app reports on launch.
        .route(
            "/notifications/devices/{id}",
            patch(devices::patch).delete(devices::delete),
        )
        // Send yourself a test. Postgres only, rate-limited by reading the
        // caller's notification rows, so any replica can take it.
        .route(
            "/notifications/test",
            post(test_notice::send_test_notification),
        )
        // The key a browser needs before it can register one. Reads three env
        // vars and no node-local disk, so it belongs on the fleet with its
        // siblings.
        .route(
            "/notifications/vapid-public-key",
            get(handlers::vapid_public_key),
        )
        .route("/notifications/{id}/read", post(handlers::mark_read))
}

const FLEET: RouteRole = RouteRole::FleetOk;

/// Every route [`routes`] mounts, and which pod may serve it: all FleetOk, as
/// `route_fleet` declared them inside `oxy-app`. Declared explicitly because
/// that type gate does not reach across the crate line. Absolute paths: the api
/// seam merges this crate at the `/api` root, and the installer adds `/api`.
pub fn route_roles() -> &'static [RouteRoleDecl] {
    &ROUTE_ROLES
}

const fn fleet(path: &'static str) -> RouteRoleDecl {
    RouteRoleDecl {
        method: "*",
        path,
        role: FLEET,
    }
}

static ROUTE_ROLES: [RouteRoleDecl; 13] = [
    fleet("/notifications"),
    fleet("/notifications/unread"),
    fleet("/notifications/catalog"),
    fleet("/notifications/{id}"),
    fleet("/notifications/{id}/actions/{action_id}"),
    fleet("/notifications/preferences"),
    fleet("/notifications/read-all"),
    fleet("/notifications/settings"),
    fleet("/notifications/devices"),
    fleet("/notifications/devices/{id}"),
    fleet("/notifications/test"),
    fleet("/notifications/vapid-public-key"),
    fleet("/notifications/{id}/read"),
];

/// The queue kinds this surface runs, for `SurfaceSeams::tasks`. `oxy-app`
/// refuses to start a process that drains the queue without them
/// (`NOTIFICATION_TASK_KINDS`): every send queues the first, and a delivery
/// that quiet hours held queues the second.
pub fn tasks() -> Vec<TaskSeam> {
    vec![
        // A send's push, email and Slack, queued in the send's own transaction
        // (`ctx.notify`'s and every platform notice's). A pod that predates the
        // kind fails the task as unknown and the retry policy lands it on one
        // that knows.
        TaskSeam {
            kind: task::NOTIFICATION_DELIVER_KIND,
            build: |deps| {
                Arc::new(task::NotificationDeliverExecutor {
                    db: deps.db.clone(),
                })
            },
        },
        // The pushes quiet hours held, told as one when the window ends. Queued
        // by a delivery run, due hours later (`available_at`); a pod without the
        // kind fails it and the holds stay in the inbox.
        TaskSeam {
            kind: settings::summary_task::QUIET_SUMMARY_KIND,
            build: |deps| {
                Arc::new(settings::summary_task::QuietSummaryExecutor {
                    db: deps.db.clone(),
                })
            },
        },
    ]
}

/// What `oxy serve` and `oxy worker` run at boot, for
/// `SurfaceSeams::notifications`.
pub fn seam() -> NotificationsSeam {
    NotificationsSeam {
        install: install_process_hooks,
        after_migrations: || Box::pin(settings::reconcile::at_boot()),
    }
}

/// The inline transport the write path below this crate delivers a
/// workspace-less send through, the agentic-runtime observers that raise run
/// notices, and the push transports; whether any push transport is configured.
fn install_process_hooks() -> bool {
    oxy_app_core::notifications::inline::install(delivery::deliver_inline);
    // A run this process drives to a terminal failure is announced to whoever
    // started it (`oxy.run_failed`); an Ask run whose asker wanted to hear when
    // it ended, done or failed, to them (`oxy.ask_finished`), and each time it
    // stops to ask them something (`oxy.ask_waiting`).
    platform::run_failed::install();
    platform::ask_finished::install();
    platform::ask_waiting::install();
    web_push::register_if_configured()
}

/// The live catalogue, as the staff console's app test reads it
/// (`oxy_app_core::notifications::catalog::live_kinds`).
pub fn live_kinds() -> Arc<dyn LiveKinds> {
    Arc::new(LiveCatalogue)
}

struct LiveCatalogue;

#[async_trait::async_trait]
impl LiveKinds for LiveCatalogue {
    async fn live_kinds(
        &self,
        db: &sea_orm::DatabaseConnection,
        app: &entity::apps::Model,
    ) -> Result<Vec<KindSummary>, sea_orm::DbErr> {
        let catalogue = catalog::live::load(db, app).await?;
        Ok(summaries(&catalogue))
    }
}

/// Every kind `catalogue` describes or a function may send, in kind order.
fn summaries(catalogue: &catalog::source::SourceCatalogue) -> Vec<KindSummary> {
    catalogue
        .kinds_with([])
        .into_iter()
        .map(|kind| {
            let info = catalogue.describe(&kind);
            KindSummary {
                kind: info.kind,
                label: info.label,
                description: info.description,
                level: info.level,
                category_labels: info.category_labels,
                mutable: info.mutable,
            }
        })
        .collect()
}

#[cfg(test)]
mod surface_tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    /// axum checks for route conflicts eagerly as a router is built.
    #[test]
    fn routes_build_without_conflict() {
        let _ = routes();
    }

    /// Every mounted path is declared, and nothing is declared that is not
    /// mounted: a route left out would take the FleetOk default by accident
    /// rather than by statement.
    #[test]
    fn every_route_is_declared_fleet_ok() {
        let declared: Vec<&str> = route_roles().iter().map(|d| d.path).collect();
        assert_eq!(declared.len(), 13);
        assert!(route_roles().iter().all(|d| d.role == RouteRole::FleetOk));
        let mut unique = declared.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), declared.len(), "a path is declared twice");
    }

    /// The persisted task-queue contract: these strings are in
    /// `agentic_task_queue` rows, and `oxy-app` refuses to start without them
    /// (`NOTIFICATION_TASK_KINDS`, which cannot name these constants).
    #[test]
    fn task_kinds_are_the_ones_oxy_app_requires() {
        let kinds: Vec<&str> = tasks().iter().map(|t| t.kind).collect();
        assert_eq!(kinds, oxy_app::surface::seams::NOTIFICATION_TASK_KINDS);
    }

    /// What the staff console's app test lists: moved here from
    /// `oxy-api-custom-apps` with the catalogue it reads.
    #[test]
    fn the_console_reads_every_kind_with_its_level_and_whether_a_mute_applies() {
        let manifest = json!({
            "notifications": {
                "kinds": {
                    "check_failed": { "label": "Check failed", "level": "time_sensitive" },
                    "fire_alarm": { "label": "Fire alarm", "level": "critical" }
                }
            },
            "functions": {
                "close": { "notify": { "kinds": { "check_failed": {}, "list_overdue": {} } } }
            }
        });
        let kinds = summaries(&catalog::live::from_manifest(Some(&manifest)));
        let by_kind: BTreeMap<_, _> = kinds.iter().map(|k| (k.kind.as_str(), k)).collect();
        assert_eq!(by_kind.len(), 3, "{kinds:?}");
        assert_eq!(by_kind["check_failed"].label, "Check failed");
        assert_eq!(by_kind["check_failed"].level.as_str(), "time_sensitive");
        assert!(by_kind["check_failed"].mutable);
        assert_eq!(by_kind["fire_alarm"].level.as_str(), "critical");
        assert!(
            !by_kind["fire_alarm"].mutable,
            "a critical kind ignores mutes"
        );
        assert!(
            by_kind["list_overdue"].mutable,
            "a kind only a function declares is listed too"
        );
        assert!(summaries(&catalog::live::from_manifest(None)).is_empty());
    }
}
