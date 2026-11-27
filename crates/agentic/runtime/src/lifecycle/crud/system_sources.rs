//! The run `source_type`s that are the platform's own background work, which
//! the run feed hides by default (`queries::list_runs_filtered`,
//! `queries::list_active_runs`).

/// Run source_types that are background daemons rather than user-scheduled
/// work. The coordinator dashboard hides them from the main feed by
/// default — they'd otherwise flood the run list at the worker's
/// heartbeat cadence (preagg_cycle fires every 30s by default).
/// Must stay in sync with the frontend constant of the same name.
///
/// Not the same list as `NON_WORKSPACE_RUN_SOURCES` in
/// `oxy-app`'s `server::api::admin::workspace_health::queries`, and not a
/// superset of it: this one answers "hide from the run feed?", that one answers
/// "does a failure here indict the tenant?". `health_eval_workspace` is excluded
/// there but stays visible here on purpose. A new daemon `source_type` should be
/// considered for both.
///
/// `preview_analyze` is the staff-only Airway change check of a previewed
/// branch (`oxy-api-previews` `analyze`), not the customer's work;
/// `preview_schema_drop` is the TTL drop of a preview's Airhouse schemas
/// (`oxy-api-previews` `drop`), queued by a maintenance sweep; `preview_compare`
/// compares a preview's transform build with live (`oxy-api-previews` `compare`);
/// `preview_airway_sample` is staff sampling a branch's Airway pipeline into
/// the preview's own schemas (`oxy-api-previews` `sample`).
/// `custom_app_staging_migrations` is a custom app's staging migrations, queued
/// by every publish (`server::api::custom_apps_nonproduction::staging_task`).
/// `custom_app_sandbox_migrations`, `custom_app_sandbox_oltp` and
/// `custom_app_sandbox_teardown` are a custom-app sandbox's Airhouse
/// migrations and its own OLTP schema (each queued by a publish to it) and its
/// teardown (queued by a delete or an expiry) —
/// `server::api::custom_apps_sandboxes`: staff work on a staff surface.
/// `custom_app_env_oltp_branch` cuts, resets and migrates a sandbox's own OLTP
/// branch (`oxy-api-custom-apps` `custom_apps_env_branch`): the same.
/// `custom_app_usage_backfill` copies a workspace's custom-app usage history to
/// its Airhouse (`oxy-api-custom-apps` `custom_apps_usage_backfill`), queued by
/// a sweep. `custom_app_usage_purge` deletes a deleted app's usage facts from
/// that Airhouse (`custom_apps_usage_purge`), queued by the delete.
/// `app_events_relay` moves an org's published custom-app events out of its
/// OLTP outbox and queues their deliveries (`oxy-api-custom-apps`
/// `app_events::relay`), queued by a publish's nudge and by a sweep: the
/// platform's plumbing, filed in one of the org's app workspaces.
/// `app_table_watch` reads a watched table's changes and publishes them as
/// events (`app_events::table_watch`), queued the same way.
/// `custom_app_build_checks` is a build's promote checks, queued by every move
/// that makes a custom app's staging serve it (`oxy-api-custom-apps`
/// `custom_apps_build_checks`): staff checking a build, not the tenant's work.
/// `custom_app_production_check_failed` is what follows a failed production
/// check of a build (the notice, and the automatic rollback where an app asks
/// for one; `custom_apps_build_checks::production`): the platform's own.
/// `custom_app_build_checks_ai` writes the advisory notes on a settled staging
/// attempt of a build's checks (`custom_apps_build_checks::ai`): staff
/// reviewing a build, not the tenant's work.
/// `react_memory_merge` shortens a user's agent notes after their chat
/// (`agentic_pipeline::queued_run::memory_merge`): upkeep no member started.
pub const SYSTEM_SOURCE_TYPES: &[&str] = &[
    "preagg_cycle",
    "preview_analyze",
    "preview_schema_drop",
    "preview_compare",
    "preview_airway_sample",
    "custom_app_staging_migrations",
    "custom_app_sandbox_migrations",
    "custom_app_sandbox_oltp",
    "custom_app_env_oltp_branch",
    "custom_app_sandbox_teardown",
    "custom_app_usage_backfill",
    "custom_app_usage_purge",
    "app_events_relay",
    "app_table_watch",
    "oltp_changes_relay",
    "custom_app_build_checks",
    "custom_app_production_check_failed",
    "custom_app_build_checks_ai",
    "react_memory_merge",
];
