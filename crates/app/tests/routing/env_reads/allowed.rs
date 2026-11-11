//! The backlog: every `OXY_*` read outside `oxy_app_core::config` on 2026-11-08,
//! one entry per variable and file. It shrinks and never grows: a new read goes
//! through the config module, and an entry whose site is gone is deleted
//! (`every_oxy_env_read_is_central_or_in_the_backlog` fails on a stale one).
//!
//! Regenerate the list from the scan if it drifts:
//! `python3 -I scripts/env-inventory.py --sites`.

/// `(variable, file, why it is still read there)`.
pub const ALLOWED: &[(&str, &str, &str)] = &[
    (
        "OXY_AIRHOUSE_OBS_INSECURE",
        "crates/cameras/src/airhouse/client.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY",
        "crates/app/src/agentic_wiring/airhouse_pool.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_AIRHOUSE_POOL_DISABLED",
        "crates/app/src/agentic_wiring/airhouse_pool.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_AIRHOUSE_POOL_IDLE_SECS",
        "crates/app/src/agentic_wiring/airhouse_pool.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_AIRHOUSE_POOL_MAX_APP_IDENTITIES",
        "crates/app/src/agentic_wiring/airhouse_pool.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_AIRHOUSE_POOL_MAX_IDENTITIES",
        "crates/app/src/agentic_wiring/airhouse_pool.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_AIRHOUSE_POOL_MAX_PREVIEW_IDENTITIES",
        "crates/app/src/agentic_wiring/airhouse_pool.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_AIRHOUSE_SNAPSHOT_WINDOW_HOURS",
        "crates/custom-apps/src/planes/airhouse.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_APNS_TOPIC",
        "crates/api-notifications/src/simulator_push.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_APP_ADMINS",
        "crates/custom-apps/src/auth.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_APP_CALL_TRACE_RETENTION_DAYS",
        "crates/custom-apps/src/call_trace_store.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_APP_EMAIL_FROM",
        "crates/app-core/src/emails/app_emailer.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_APP_EMAIL_LOCAL_TEST",
        "crates/app-core/src/emails/app_emailer.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_APP_EMAIL_REGION",
        "crates/app-core/src/emails/app_emailer.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_APP_ENV_OLTP_BRANCHES_MAX",
        "crates/oltp/src/env_branches.rs",
        "OLTP provider setting (`oltp::config` groups its own)",
    ),
    (
        "OXY_APP_SANDBOX_IDLE_DAYS",
        "crates/api-custom-apps/src/custom_apps_sandboxes/mod.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_APP_SANDBOX_MAINTENANCE_INTERVAL_SECS",
        "crates/api-custom-apps/src/custom_apps_sandboxes/maintenance.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_APP_SANDBOX_OLTP_SEED_MAX_ROWS",
        "crates/oltp/src/sandbox_schema/seed.rs",
        "OLTP provider setting (`oltp::config` groups its own)",
    ),
    (
        "OXY_APP_SANDBOX_OLTP_SEED_MAX_TABLE_BYTES",
        "crates/oltp/src/sandbox_schema/seed.rs",
        "OLTP provider setting (`oltp::config` groups its own)",
    ),
    (
        "OXY_APP_SANDBOX_OLTP_SEED_MAX_TOTAL_BYTES",
        "crates/oltp/src/sandbox_schema/seed.rs",
        "OLTP provider setting (`oltp::config` groups its own)",
    ),
    (
        "OXY_AUDIT_ANCHOR_INTERVAL_SECS",
        "crates/app/src/server/audit_anchor.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_AUDIT_ANCHOR_KEY_PREFIX",
        "crates/app/src/server/audit_anchor.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_AUDIT_ANCHOR_RETENTION_DAYS",
        "crates/app/src/server/audit_anchor.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_AUDIT_ANCHOR_S3_BUCKET",
        "crates/app/src/server/audit_anchor.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_AUTH_ALLOW_LOCALHOST_RETURN",
        "crates/app/src/server/api/auth/ops.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_BIN",
        "crates/test-utils/src/lib.rs",
        "read at the call site; migrate when the module is next changed",
    ),
    (
        "OXY_BUNDLE_CACHE_MAX_MB",
        "crates/custom-apps/src/bundle_cache.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CAMERAS_AIRHOUSE_CONNECT_TIMEOUT_SECS",
        "crates/cameras/src/airhouse/mod.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_AIRHOUSE_INGEST_TTL_SECS",
        "crates/cameras/src/airhouse/mod.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_AIRHOUSE_INSERT_CHUNK_ROWS",
        "crates/cameras/src/airhouse/mod.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_AIRHOUSE_MAX_RECONNECT_ATTEMPTS",
        "crates/cameras/src/airhouse/mod.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_AIRHOUSE_READ_TTL_SECS",
        "crates/cameras/src/airhouse/mod.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_BUFFER_MAX_ROWS",
        "crates/cameras/src/service/ingest_buffer.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_CLIPS_S3_BUCKET",
        "crates/cameras/src/service/clips.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_CLIPS_S3_KEY_PREFIX",
        "crates/cameras/src/service/clips.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_CLIPS_S3_PRESIGN_TTL_SECONDS",
        "crates/cameras/src/service/clips.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_CLIPS_S3_REGION",
        "crates/cameras/src/service/clips.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_EDGE_HLS_PORT",
        "crates/cameras/src/service/preview.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_EDGE_PLAYBACK_PORT",
        "crates/cameras/src/service/preview.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_EDGE_PREVIEW_PORT",
        "crates/cameras/src/service/preview.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_FLUSH_INTERVAL_MS",
        "crates/cameras/src/service/ingest_buffer.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_FLUSH_MAX_ROWS",
        "crates/cameras/src/service/ingest_buffer.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_INGEST_BUFFER_DISABLED",
        "crates/cameras/src/service/ingest_buffer.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_INGEST_MAX_FLUSH_ATTEMPTS",
        "crates/cameras/src/service/ingest_buffer.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_TURN_AUTH_SECRET",
        "crates/cameras/src/routes/mtx_auth.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERAS_TURN_AUTH_SECRET",
        "crates/cameras/src/routes/operator.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERA_HEALTH_DECODER_ERROR_LIMIT",
        "crates/cameras/src/service/camera_health.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERA_HEALTH_FPS_TARGET",
        "crates/cameras/src/service/camera_health.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERA_HEALTH_RECONNECT_LIMIT",
        "crates/cameras/src/service/camera_health.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERA_HEALTH_STALE_FRAME_SECS",
        "crates/cameras/src/service/camera_health.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERA_HEALTH_STALE_REPORT_SECS",
        "crates/cameras/src/service/camera_health.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERA_LOG_RETENTION_INFO_DAYS",
        "crates/cameras/src/service/log_retention.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERA_LOG_RETENTION_WARN_DAYS",
        "crates/cameras/src/service/log_retention.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CAMERA_LOG_SWEEP_INTERVAL_HOURS",
        "crates/cameras/src/service/log_retention.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_CLICKHOUSE_DATABASE",
        "crates/observability/src/backends/clickhouse/mod.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_CLICKHOUSE_PASSWORD",
        "crates/observability/src/backends/clickhouse/mod.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_CLICKHOUSE_URL",
        "crates/observability/src/backends/clickhouse/mod.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_CLICKHOUSE_USER",
        "crates/observability/src/backends/clickhouse/mod.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_COMPILE_BLOB_S3_BUCKET",
        "crates/oxy-compile/src/blob_store.rs",
        "read at the call site; migrate when the module is next changed",
    ),
    (
        "OXY_COMPILE_MAINTENANCE_INTERVAL_SECS",
        "crates/app/src/server/compile_maintenance.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_COMPILE_RECONCILE",
        "crates/app/src/server/feature_flags/rollout.rs",
        "deprecated env override for one release; delete with the flag's env override (feature-flags.md, section Rollout gates and their sunset)",
    ),
    (
        "OXY_COMPILE_REFUSE_FILE_CREDENTIALS",
        "crates/app/src/server/feature_flags/rollout.rs",
        "deprecated env override for one release; delete with the flag's env override (feature-flags.md, section Rollout gates and their sunset)",
    ),
    (
        "OXY_COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS",
        "crates/app/src/server/feature_flags/rollout.rs",
        "deprecated env override for one release; delete with the flag's env override (feature-flags.md, section Rollout gates and their sunset)",
    ),
    (
        "OXY_COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS",
        "crates/oxy-compile/src/automation_check.rs",
        "fallback for a compile with no ConfigGate (oxy-compile cannot read flags); delete with the env override",
    ),
    (
        "OXY_COMPILE_RETENTION_DAYS",
        "crates/app/src/server/compile_maintenance.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_COMPILE_STUCK_TIMEOUT_SECS",
        "crates/app/src/server/compile_maintenance.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_CUSTOMER_APPS_FETCH_MAX_BYTES",
        "crates/api-custom-apps/src/custom_apps_functions/mod.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOMER_APPS_MAX_APPS_PER_ORG",
        "crates/api-custom-apps/src/custom_apps_publish/limits.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOMER_APPS_MAX_PUBLISHES_PER_HOUR",
        "crates/api-custom-apps/src/custom_apps_publish/limits.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOMER_APPS_REPO",
        "crates/api-custom-apps/src/custom_apps_scaffold.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOMER_APPS_S3_BUCKET",
        "crates/custom-apps/src/build_store.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOMER_APPS_STORAGE_ENVIRONMENT_MAX_BYTES",
        "crates/custom-apps/src/storage/environment_limits.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOMER_APPS_STORAGE_HARD_LIMIT_BYTES",
        "crates/custom-apps/src/storage/quota.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOMER_APPS_STORAGE_MAX_UPLOAD_BYTES",
        "crates/custom-apps/src/storage/mod.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOMER_APPS_STORAGE_S3_BUCKET",
        "crates/custom-apps/src/storage/mod.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOMER_APPS_STORAGE_SOFT_LIMIT_BYTES",
        "crates/custom-apps/src/storage/quota.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOMER_APPS_STORAGE_SWEEP_BATCH",
        "crates/custom-apps/src/storage/sweeper.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOMER_APPS_STORAGE_SWEEP_INTERVAL_SECS",
        "crates/custom-apps/src/storage/sweeper.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOM_APP_UPDATES",
        "crates/custom-apps/src/updates.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOM_DOMAIN_DOH_URL",
        "crates/custom-apps/src/custom_domains/dns.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_CUSTOM_DOMAIN_SCHEME",
        "crates/api-custom-apps/src/custom_domains/handoff.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_DATABASE_AUTH_MODE",
        "crates/platform/src/db/auth_mode.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_DATABASE_HOST",
        "crates/server/src/commands/worker.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_DATABASE_HOST",
        "crates/platform/src/db/auth_mode.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_DATABASE_MAX_CONNECTIONS",
        "crates/platform/src/db/client.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_DATABASE_MIN_CONNECTIONS",
        "crates/platform/src/db/client.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_DATABASE_NAME",
        "crates/server/src/commands/worker.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_DATABASE_NAME",
        "crates/platform/src/db/auth_mode.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_DATABASE_PORT",
        "crates/platform/src/db/auth_mode.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_DATABASE_REGION",
        "crates/platform/src/db/auth_mode.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_DATABASE_REGION",
        "crates/platform/src/secrets/master_key/aws.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_DATABASE_SSL_MODE",
        "crates/platform/src/db/auth_mode.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_DATABASE_URL",
        "crates/app/src/cli/commands/seed_partners.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_DATABASE_URL",
        "crates/server/src/commands/serve.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_DATABASE_URL",
        "crates/server/src/commands/worker.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_DATABASE_URL",
        "crates/app/src/server/test_support.rs",
        "test-only helper: the shared database of in-source `#[cfg(test)]` tests",
    ),
    (
        "OXY_DATABASE_URL",
        "crates/migration/src/main.rs",
        "read at the call site; migrate when the module is next changed",
    ),
    (
        "OXY_DATABASE_URL",
        "crates/oltp/src/provider/local.rs",
        "OLTP provider setting (`oltp::config` groups its own)",
    ),
    (
        "OXY_DATABASE_URL",
        "crates/platform/src/db/client.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_DATABASE_URL",
        "crates/platform/src/db/listener.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_DATABASE_URL",
        "crates/test-utils/src/containers/mod.rs",
        "test-only helper (dev-dep crate below oxy-app-core): CI's database for the shared test Postgres",
    ),
    (
        "OXY_DATABASE_USER",
        "crates/platform/src/db/auth_mode.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_DEBUG",
        "crates/server/src/logging.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_DEV_LOGIN_EMAILS",
        "crates/app/src/server/api/auth/dev_login.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_DISABLE_INPROCESS_WORKERS",
        "crates/server/src/commands/args.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_DOCUMENT_AGENT",
        "crates/api-documents/src/ask_agent.rs",
        "read at the call site; migrate when the module is next changed",
    ),
    (
        "OXY_EDGE_INSTALL_URL",
        "crates/cameras/src/routes/operator.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_ENCRYPTION_KEY",
        "crates/platform/src/secrets/encryption.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_FCM_SERVICE_ACCOUNT_JSON",
        "crates/api-notifications/src/fcm.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_FEATURE_FLAG_REFRESH_MS",
        "crates/app/src/server/feature_flags/cache.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_FLEET_CLAIM_SALT",
        "crates/cameras/src/routes/operator.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_FUNCTION_HEAP_LIMIT_MB",
        "crates/api-custom-apps/src/custom_apps_functions/runtime/isolate_limits.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_FUNCTION_MAX_CONCURRENCY",
        "crates/api-custom-apps/src/custom_apps_functions/limits.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_FUNCTION_MAX_ORG_CONCURRENCY",
        "crates/api-custom-apps/src/custom_apps_functions/limits.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_FUNCTION_MAX_QUEUED",
        "crates/api-custom-apps/src/custom_apps_functions/limits.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_FUNCTION_QUEUE_BUDGET_SECS",
        "crates/api-custom-apps/src/custom_apps_functions/limits.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_GIT_FETCH_INTERVAL_SECS",
        "crates/app/src/server/git_fetch_maintenance.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_GLOBAL_ADMINS",
        "crates/app/src/cli/commands/seed.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_GLOBAL_ADMINS",
        "crates/app/src/server/api/auth/dev_login.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_GLOBAL_ADMINS",
        "crates/custom-apps/src/auth.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_HEALTH_FAIL_RATE_DEGRADED",
        "crates/workspace-health/src/evaluator.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_HEALTH_FAIL_RATE_UNHEALTHY",
        "crates/workspace-health/src/evaluator.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_HEALTH_MIN_RUNS",
        "crates/workspace-health/src/evaluator.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_HEALTH_REALERT_HOURS",
        "crates/workspace-health/src/alert.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_HEALTH_WINDOW_HOURS",
        "crates/workspace-health/src/evaluator.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_IDE_DEFER_AIRWAY",
        "crates/app/src/server/router/drive_policy.rs",
        "the ide's queue-selection rollout gates; make them `Rollout` registry flags when next touched (worker-fleet.md)",
    ),
    (
        "OXY_IDE_DEFER_QUEUE_WORK",
        "crates/app/src/server/router/drive_policy.rs",
        "the ide's queue-selection rollout gates; make them `Rollout` registry flags when next touched (worker-fleet.md)",
    ),
    (
        "OXY_IDE_UPSTREAM",
        "crates/app/src/server/ide_proxy.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_IMAGE_TAG",
        "crates/shared/src/image_tag.rs",
        "below oxy-app-core in the dependency graph; a config of its own crate when next touched",
    ),
    (
        "OXY_INPROC_GLOBAL_WORKER",
        "crates/app/src/server/router/recovery.rs",
        "#3990 edits this file: migrate with it",
    ),
    (
        "OXY_INPROC_GLOBAL_WORKER_INTERVAL_SECS",
        "crates/app/src/server/router/recovery.rs",
        "#3990 edits this file: migrate with it",
    ),
    (
        "OXY_LATENCY_WORKER_INTERVAL_MS",
        "crates/app/src/server/router/recovery.rs",
        "#3990 edits this file: migrate with it",
    ),
    (
        "OXY_LOG_FORMAT",
        "crates/server/src/logging.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_LOG_LEVEL",
        "crates/observability/src/telemetry.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_LOG_LEVEL",
        "crates/server/src/logging.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_MAX_INFLIGHT_PER_WORKSPACE",
        "crates/app/src/server/admission.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_MAX_INFLIGHT_REQUESTS",
        "crates/app/src/server/admission.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_METRICS_PORT",
        "crates/app/src/server/metrics_server.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_MIN_EDGE_PROTOCOL_VERSION",
        "crates/cameras/src/service/updates.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_OAUTH_REDIRECT_ORIGIN",
        "crates/api-github/src/state.rs",
        "read at the call site; migrate when the module is next changed",
    ),
    (
        "OXY_OAUTH_REDIRECT_ORIGIN",
        "crates/app/src/server/api/auth/ops.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_OBSERVABILITY_BACKEND",
        "crates/server/src/commands/start.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_OBSERVABILITY_BACKEND",
        "crates/app/src/observability_boot.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_OBSERVABILITY_BACKEND",
        "crates/app/src/observability_setup.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_OBSERVABILITY_BACKEND",
        "crates/server/src/main.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_OBSERVABILITY_LOG_LEVEL",
        "crates/observability/src/telemetry.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_OLTP_ADMIN_URL",
        "crates/server/src/commands/start.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_OLTP_ADMIN_URL",
        "crates/oltp/src/config.rs",
        "OLTP provider setting (`oltp::config` groups its own)",
    ),
    (
        "OXY_OLTP_BRANCH_HISTORY_HOURS",
        "crates/oltp/src/env_branches.rs",
        "OLTP provider setting (`oltp::config` groups its own)",
    ),
    (
        "OXY_OLTP_NEON_API_KEY",
        "crates/oltp/src/config.rs",
        "OLTP provider setting (`oltp::config` groups its own)",
    ),
    (
        "OXY_OLTP_NEON_ORG_ID",
        "crates/oltp/src/config.rs",
        "OLTP provider setting (`oltp::config` groups its own)",
    ),
    (
        "OXY_OLTP_PG_VERSION",
        "crates/oltp/src/config.rs",
        "OLTP provider setting (`oltp::config` groups its own)",
    ),
    (
        "OXY_OLTP_PROVIDER",
        "crates/server/src/commands/start.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_OLTP_PROVIDER",
        "crates/oltp/src/config.rs",
        "OLTP provider setting (`oltp::config` groups its own)",
    ),
    (
        "OXY_OLTP_REGION",
        "crates/oltp/src/config.rs",
        "OLTP provider setting (`oltp::config` groups its own)",
    ),
    (
        "OXY_OPS_ALLQUIET_FUNCTION_FAILURES_URL",
        "crates/api-custom-apps/src/custom_apps_functions/failure_incident.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_OPS_SLACK_BOT_TOKEN",
        "crates/workspace-health/src/eval_pass.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_OPS_SLACK_CHANNEL",
        "crates/workspace-health/src/eval_pass.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_OTEL_FILTER",
        "crates/telemetry/src/otel.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_OWNER",
        "crates/server-authz/src/oxy_owner_guard.rs",
        "read at the call site; migrate when the module is next changed",
    ),
    (
        "OXY_PREVIEW_COW_MAX_ROWS",
        "crates/airhouse/src/preview_sql/copy.rs",
        "below oxy-app-core in the dependency graph; a config of its own crate when next touched",
    ),
    (
        "OXY_PREVIEW_DIFF_MAX_ROWS",
        "crates/app/src/server/previews/compare/table.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_PREVIEW_MAINTENANCE_INTERVAL_SECS",
        "crates/app/src/server/previews/maintenance.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_PREVIEW_RUNS",
        "crates/app/src/server/previews/runs.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_PREVIEW_RUN_MAX_MINUTES",
        "crates/app/src/server/previews/runs/ceiling.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_PREVIEW_SAMPLE_MAX_SECS",
        "crates/app/src/server/previews/sample.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_PREVIEW_SCHEMA_TTL_HOURS",
        "crates/app/src/server/previews/registry/mod.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_PREVIEW_SQL_CORPUS",
        "crates/app-core/src/sql_kind_corpus.rs",
        "test-only module (`#[cfg(test)] #[path]` in sql_kind.rs): a hand-run corpus",
    ),
    (
        "OXY_PUBLIC_URL",
        "crates/cameras/src/routes/operator.rs",
        "cameras domain setting; migrate into a cameras config in that crate",
    ),
    (
        "OXY_RECONCILE_PCT_UNHEALTHY",
        "crates/api-workspace-health/src/reconcile_runner.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_RECONCILE_REPORT_TIMEOUT_SECS",
        "crates/api-workspace-health/src/reconcile_runner.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_REQUEST_TIMEOUT_SECS",
        "crates/app/src/server/api/middlewares/timeout.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_ROLE",
        "crates/platform/src/db/client.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_ROLE",
        "crates/platform/src/secrets/encryption.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_ROLE",
        "crates/server/src/main.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_SEED_ALLOW_REMOTE",
        "crates/app/src/cli/commands/seed_partners.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_SERVICE_NAME",
        "crates/observability/src/telemetry.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_SESSION_COOKIE_DOMAIN",
        "crates/api-frontline/src/frontline_kiosk_cookie.rs",
        "read at the call site; migrate when the module is next changed",
    ),
    (
        "OXY_SESSION_COOKIE_DOMAIN",
        "crates/app/src/server/api/auth/ops.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_SESSION_COOKIE_FORCE_SECURE",
        "crates/app/src/server/api/auth/ops.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_SKIP_MIGRATIONS",
        "crates/app/src/cli/commands/migrate.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_SMOKE_AGENT_TIMEOUT_SECS",
        "crates/workspace-health/src/smoke/config.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_SMOKE_CONCURRENCY",
        "crates/workspace-health/src/smoke/config.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_SMOKE_PROBE_TIMEOUT_SECS",
        "crates/workspace-health/src/smoke/config.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_SMOKE_TOTAL_TIMEOUT_SECS",
        "crates/workspace-health/src/smoke/config.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_SOURCE_UPLOAD_ZONE",
        "crates/agentic/airway/src/upload_zone.rs",
        "below oxy-app-core in the dependency graph; a config of its own crate when next touched",
    ),
    (
        "OXY_STAGING_AIRHOUSE_MIGRATION_DEADLINE_SECS",
        "crates/api-custom-apps/src/custom_apps_nonproduction/staging_task_executor.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_STATE_DIR",
        "crates/core/src/config/storage.rs",
        "below oxy-app-core in the dependency graph; a config of its own crate when next touched",
    ),
    (
        "OXY_STATE_DIR",
        "crates/custom-apps/src/build_store.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_STATE_DIR",
        "crates/custom-apps/src/storage/local.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_STATE_DIR",
        "crates/platform/src/secrets/encryption.rs",
        "platform infrastructure (Postgres, secrets) below oxy-app-core; migrate when oxy-platform grows a config",
    ),
    (
        "OXY_STATE_DIR",
        "crates/shared/src/state_dir.rs",
        "below oxy-app-core in the dependency graph; a config of its own crate when next touched",
    ),
    (
        "OXY_STDERR_CAPTURE",
        "crates/telemetry/src/stderr_capture.rs",
        "telemetry and logging bootstrap, read before the app layer exists",
    ),
    (
        "OXY_TASK_QUEUE_DEAD_RETENTION_DAYS",
        "crates/agentic/runtime/src/orchestrator/background.rs",
        "below oxy-app-core in the dependency graph; a config of its own crate when next touched",
    ),
    (
        "OXY_TASK_QUEUE_RETENTION_DAYS",
        "crates/agentic/runtime/src/orchestrator/background.rs",
        "below oxy-app-core in the dependency graph; a config of its own crate when next touched",
    ),
    (
        "OXY_TASK_QUEUE_RETENTION_INTERVAL_SECS",
        "crates/agentic/runtime/src/orchestrator/background.rs",
        "below oxy-app-core in the dependency graph; a config of its own crate when next touched",
    ),
    (
        "OXY_TOAST_BASE_URL",
        "crates/workspace-health/src/reconcile/toast/mod.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_UPDATE_GOLDEN",
        "crates/api-custom-apps/src/custom_apps_functions/env_policy/golden.rs",
        "custom-apps domain setting; migrate into oxy-custom-apps' own config",
    ),
    (
        "OXY_VAPID_PRIVATE_KEY",
        "crates/api-notifications/src/web_push.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_VAPID_PUBLIC_KEY",
        "crates/api-notifications/src/web_push.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_VAPID_SUBJECT",
        "crates/api-notifications/src/web_push.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_WORKER_HEALTH_PORT",
        "crates/server/src/commands/worker.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_WORKER_MAX_INFLIGHT",
        "crates/agentic/runtime/src/orchestrator/worker.rs",
        "below oxy-app-core in the dependency graph; a config of its own crate when next touched",
    ),
    (
        "OXY_WORKER_MAX_INFLIGHT",
        "crates/app/src/server/worker_metrics.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_WORKER_RECOVERY_INTERVAL_SECS",
        "crates/server/src/commands/worker.rs",
        "CLI command switch read at the call site; migrate with the command's next change",
    ),
    (
        "OXY_WORKTREE_IDLE_TIMEOUT_SECS",
        "crates/app/src/server/worktree_registry.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
    (
        "OXY_WORKTREE_SWEEP_INTERVAL_SECS",
        "crates/app/src/server/worktree_registry.rs",
        "oxy-app read at the call site with its default; migrate to BootConfig when the module is next changed",
    ),
];

/// Files a sibling PR is adding or rewriting, allowed whole (prefix match) and
/// allowed to be absent, so neither PR breaks the other's build when it merges.
/// Delete an entry once that PR has landed and its reads are listed above or
/// migrated.
pub const ALLOWED_PENDING_PREFIXES: &[(&str, &str)] = &[
    (
        "crates/app-core/src/rate_limit.rs",
        "#3989: the rate limiter; migrate with it",
    ),
    (
        "crates/app/src/server/router/periodic/",
        "#3990: the periodic driver; migrate with it",
    ),
];
