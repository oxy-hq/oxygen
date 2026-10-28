//! Fails the build when a table lands in Oxy's own Postgres without a stated
//! reason to be there.
//!
//! ## Why a test
//!
//! Oxy is the control plane and each org is its own data plane
//! (`internal-docs/data-placement.md`). Nothing about adding a table enforces
//! that. A migration in `crates/migration` is the shortest path for any feature,
//! which is how `world_model_events` came to hold every org's POS orders, and
//! chat, work items, notifications and documents came to live beside the user
//! table. So the objection has to be mechanical: every base table the migrators
//! create is named below, as `Control` with a reason, or as `Backlog` with the
//! store its data's shape picks.
//!
//! ## The backlog is the migration, not an exemption list
//!
//! A `Backlog` row is org data still in Oxy's database. It is supposed to exist
//! today and supposed to disappear: moving the table out and deleting its row is
//! the work. A new table is `Control` only if the rule in the doc says so;
//! anything else belongs in the org's data plane from its first day, and adding
//! it here as backlog is a reviewable admission that it does not.
//!
//! ## Checked against a migrated database, not source
//!
//! Tables come from `information_schema` after every migrator `oxy serve` runs,
//! so raw-SQL migrations, renames and drops are all accounted for without
//! parsing a builder chain. Checked both ways: an unlisted table fails, and so
//! does a row naming a table that no longer exists, so the list cannot drift
//! into describing a database nobody has.
//!
//! Run with: `cargo nextest run -p oxy-app --test platform -E 'test(data_placement)'`

use std::collections::BTreeSet;

use sea_orm::{ConnectionTrait, DatabaseBackend, Statement};

use crate::common::{Schema, fresh_db};

enum Placement {
    /// Belongs in Oxy's database. The reason is what a reviewer checks.
    Control(&'static str),
    /// Org data still in Oxy's database: what it holds, and where it belongs.
    Backlog {
        holds: &'static str,
        belongs: &'static str,
    },
}

use Placement::{Backlog, Control};

const TABLES: &[(&str, Placement)] = &[
    (
        "admin_assume_sessions",
        Control("staff impersonation sessions"),
    ),
    ("agent_definitions", Control("compiled .agentic.yml")),
    (
        "agentic_run_events",
        Backlog {
            holds: "run event log with payloads: LLM output, tool and SQL results",
            belongs: "payloads → org OLTP live, Airhouse for history; sequence and type stay",
        },
    ),
    (
        "agentic_run_suspensions",
        Backlog {
            holds: "resume checkpoint with prompt and suggestions",
            belongs: "prompt → org OLTP; the checkpoint stays",
        },
    ),
    (
        "agentic_runs",
        Backlog {
            holds: "run lifecycle and lease, with question, answer and error",
            belongs: "question and answer → org OLTP; lease and status stay",
        },
    ),
    ("agentic_schedules", Control("cron schedules")),
    (
        "agentic_task_outcomes",
        Backlog {
            holds: "child task status with its answer",
            belongs: "answer → org OLTP; status stays",
        },
    ),
    ("agentic_task_queue", Control("durable task queue")),
    (
        "agentic_workflow_state",
        Backlog {
            holds: "step checkpoint with results and variables",
            belongs: "results → org OLTP; the checkpoint stays",
        },
    ),
    (
        "airhouse_tenants",
        Control("Airhouse tenant registry and sealed service account"),
    ),
    (
        "airway_deployment_config",
        Control("Airway global operating config"),
    ),
    ("airway_load_audit", Control("load audit rows")),
    ("airway_pipeline_leases", Control("single-flight lease")),
    (
        "airway_pipeline_state",
        Control("incremental cursor and schema"),
    ),
    ("airway_pipelines", Control("compiled .airway.yml")),
    ("airway_run_extensions", Control("per-run Airway metadata")),
    (
        "airway_source_config",
        Control("per-source admission policy"),
    ),
    (
        "airway_workspace_pipeline_state",
        Control("incremental cursor and schema, per workspace"),
    ),
    (
        "analytics_run_extensions",
        Control("agent id and spec hint per run"),
    ),
    (
        "api_keys",
        Control("legacy API keys, plaintext until hashed into api_tokens"),
    ),
    (
        "api_token_grants",
        Control("which orgs and workspaces an API token covers, and its role ceiling"),
    ),
    (
        "api_token_usage_daily",
        Control("per-token daily request counts; ids and route templates, no content"),
    ),
    (
        "api_tokens",
        Control("API credential hashes and reach flags"),
    ),
    ("app_admin_scope_orgs", Control("platform grant org scope")),
    ("app_admins", Control("platform staff grants")),
    (
        "app_builds",
        Control("custom-app build pointers; bundles live in S3"),
    ),
    (
        "app_custom_domains",
        Control("which app a customer-owned hostname routes to, and its DNS proof; no content"),
    ),
    ("app_definitions", Control("compiled .app.yml")),
    (
        "app_email_sends",
        Control("ctx.email.send idempotency claims: the key's hash and SES message id"),
    ),
    (
        "app_environment_events",
        Control("custom-app environment pointer history; ids only"),
    ),
    (
        "app_environment_tails",
        Control("who is live-tailing a custom-app environment, as a lease; no content"),
    ),
    (
        "app_environments",
        Control("custom-app environment build pointers"),
    ),
    (
        "app_function_failure_alerts",
        Control("at-most-once failure page ledger"),
    ),
    (
        "app_function_invocation_calls",
        Control(
            "per-call timing of an invocation: registry op, offset, duration, error class and the \
             identifiers its F2 span records; never SQL, values, URL paths or bodies. The console \
             copy of the platform's own spans, kept here rather than in the product ClickHouse \
             store because product capture is opt-in and off on a dev box, where the waterfall \
             would go dark; bounded by 200 calls a row and a 3-day prune",
        ),
    ),
    (
        "app_function_invocations",
        Backlog {
            holds: "invocation bookkeeping with result_body and error text",
            belongs: "result_body → org OLTP; bookkeeping stays",
        },
    ),
    ("app_functions", Control("functions per build")),
    ("app_members", Control("per-app membership")),
    ("app_publish_tokens", Control("CI publish token hashes")),
    ("app_publishers", Control("OIDC trusted publishers")),
    (
        "app_storage_usage",
        Control("app storage size rollup, for billing"),
    ),
    (
        "app_storage_usage_samples",
        Control("periodic storage size samples, for billing"),
    ),
    ("app_team_grants", Control("app access per team")),
    ("apps", Control("custom app registry")),
    (
        "artifacts",
        Backlog {
            holds: "legacy agent artifact content per message",
            belongs: "org OLTP, or drop with the legacy thread model",
        },
    ),
    ("audit_events", Control("privileged action audit")),
    (
        "automation_definitions",
        Control("compiled .automation.yml"),
    ),
    ("backfill_checkpoints", Control("backfill chunk status")),
    ("backfill_ranges", Control("user backfill windows")),
    ("camera_audit_events", Control("camera operator audit")),
    (
        "camera_domain_packs",
        Control("camera roles, VLM prompts and schema"),
    ),
    ("camera_rollout_plans", Control("edge canary rollouts")),
    ("cameras", Control("camera fleet inventory")),
    (
        "chat_channel_members",
        Backlog {
            holds: "chat channel membership",
            belongs: "org OLTP, with the rest of chat; an app-level gate, not an authz fact",
        },
    ),
    (
        "chat_channels",
        Backlog {
            holds: "chat channels",
            belongs: "org OLTP",
        },
    ),
    (
        "chat_messages",
        Backlog {
            holds: "chat message bodies",
            belongs: "org OLTP",
        },
    ),
    (
        "cli_auth_codes",
        Control("hashed single-use codes for the oxyc login exchange"),
    ),
    (
        "cli_device_requests",
        Control("pending oxyc device-code approvals, hashed and swept within the hour"),
    ),
    (
        "compiled_references",
        Control("compiled cross-entity reference graph"),
    ),
    (
        "compliance_arbitrations",
        Backlog {
            holds: "operator verdicts on camera compliance reports",
            belongs: "Airhouse, beside the reports they judge",
        },
    ),
    (
        "context_document_definitions",
        Control("compiled .md context documents an agent's context: reaches"),
    ),
    (
        "custom_app_event",
        Backlog {
            holds: "app SDK events with a free-form payload",
            belongs: "Airhouse. The usage drain and record_event already append these rows to \
                      the workspace's oxy_custom_app_usage, a backfill copies the history, and \
                      every reader (oxy_custom_apps::usage_reads, the only one) reads it for a \
                      backfilled workspace. These rows go once Airhouse has retention and no \
                      app is left without an Airhouse (the counting SQL is in \
                      data-placement.md)",
        },
    ),
    (
        "custom_app_migrations",
        Control("applied schema ledger per app"),
    ),
    (
        "custom_app_usage_backfill",
        Control(
            "per workspace, how far copying its usage history into its Airhouse has got: \
             timestamps, a count, a resume position and when rows were last parked in \
             Postgres after a failed Airhouse append, no usage content; the row that says \
             finished moves the workspace's usage reads and writes to Airhouse",
        ),
    ),
    (
        "custom_app_usage_report_deliveries",
        Control("which staff address was sent each usage report"),
    ),
    (
        "custom_app_usage_reports",
        Control("weekly app usage rollup: counts with org and app names, no content"),
    ),
    (
        "custom_app_view_event",
        Backlog {
            holds: "who opened which app: viewer email, session, role snapshot",
            belongs: "Airhouse. The usage drain already appends these to the workspace's \
                      oxy_custom_app_usage (W2.3b), a backfill copies the history, and the \
                      readers (oxy_custom_apps::usage_reads, the only ones) read it for a \
                      backfilled workspace; the role snapshot is resolved from control-plane \
                      tables when the row is written and travels with the fact. These rows go \
                      once Airhouse has retention and no app is left without an Airhouse (the \
                      counting SQL is in data-placement.md)",
        },
    ),
    (
        "customer_app_automation_runs",
        Backlog {
            holds: "run status with params, result summary and outputs",
            belongs: "outputs → org OLTP; status stays",
        },
    ),
    (
        "document_ask_sessions",
        Backlog {
            holds: "a person's conversations with the document library",
            belongs: "org OLTP",
        },
    ),
    (
        "document_ask_turns",
        Backlog {
            holds: "questions asked of the library and the answers given",
            belongs: "org OLTP",
        },
    ),
    (
        "document_categories",
        Backlog {
            holds: "the org's document category names",
            belongs: "org OLTP",
        },
    ),
    (
        "document_favorites",
        Backlog {
            holds: "one person's bookmarks on documents",
            belongs: "org OLTP",
        },
    ),
    (
        "document_versions",
        Backlog {
            holds: "each saved version of a document and what it said",
            belongs: "org OLTP; file bytes → app storage",
        },
    ),
    (
        "documents",
        Backlog {
            holds: "the org's knowledge-base and compliance documents",
            belongs: "org OLTP; file bytes → app storage",
        },
    ),
    ("device_claims", Control("device to workspace binding")),
    ("device_registry", Control("factory device identity")),
    ("device_tokens", Control("push delivery tokens")),
    ("edge_boxes", Control("edge box inventory")),
    (
        "experiment_designs",
        Control("insert-only versions of an experiment's design: the pre-registration record"),
    ),
    (
        "experiment_proposals",
        Control(
            "pre-registration proposal log: the request, seed, assignment and MDE of every candidate design priced",
        ),
    ),
    (
        "experiment_flips",
        Control("experiment lever flips: one row per run, unit, state and due day"),
    ),
    (
        "experiment_interims",
        Backlog {
            holds: "a live run's latest interim effect, daily target series and report JSON",
            belongs: "org OLTP; it is replaced on every reading",
        },
    ),
    (
        "experiment_results",
        Backlog {
            holds: "per-run measured effect, daily target series, compliance and report JSON",
            belongs: "Airhouse (facts: the sealed report and its series)",
        },
    ),
    (
        "experiment_runs",
        Control("experiment run lifecycle, pinned to one design version"),
    ),
    (
        "experiments",
        Control("experiment identity: one name per workspace"),
    ),
    ("feature_flags", Control("feature flags")),
    (
        "folders",
        Backlog {
            holds: "the org's document folder tree",
            belongs: "org OLTP",
        },
    ),
    ("git_namespaces", Control("git provider installs")),
    ("github_accounts", Control("user GitHub OAuth")),
    (
        "location_external_ids",
        Control(
            "what each integration calls a place; the semantic binding and reach read it. Judgement call — review with the operating graph",
        ),
    ),
    (
        "locations",
        Control(
            "the place axis reach is computed on. Judgement call — review with the operating graph",
        ),
    ),
    (
        "logs",
        Backlog {
            holds: "legacy LLM prompts and log text per thread",
            belongs: "org OLTP, or drop with the legacy thread model",
        },
    ),
    (
        "messages",
        Backlog {
            holds: "legacy thread message content",
            belongs: "org OLTP",
        },
    ),
    (
        "metric_anomalies",
        Backlog {
            holds: "observed and expected values, segment filters, explanations and inbox status",
            belongs: "values → Airhouse; inbox state → org OLTP",
        },
    ),
    (
        "metric_anomaly_notifications",
        Control("at-most-once insight announcement ledger"),
    ),
    (
        "metric_monitor_coverage",
        Control("scan coverage bookkeeping per segment"),
    ),
    ("monitor_configs", Control("compiled .monitor.yml")),
    (
        "notification_deliveries",
        Control("per-channel delivery claims: ids, status and a reason code"),
    ),
    (
        "notification_mutes",
        Control("which channels a person turned off, per app and kind"),
    ),
    (
        "notification_node_settings",
        Control(
            "what a person set at each node of a notification source: channels on or off, a floor",
        ),
    ),
    (
        "notification_platform_mutes",
        Control("which channels a person turned off, per platform notice kind"),
    ),
    (
        "notification_scope_mutes",
        Control("which channels a person turned off for a whole source or a category subtree"),
    ),
    (
        "notification_sends",
        Control("one row per notify call: ids, kind, counts, the idempotency key's hash"),
    ),
    (
        "notification_user_settings",
        Control("a person's notification pause, quiet hours, timezone and settings version"),
    ),
    (
        "notifications",
        Backlog {
            holds: "legacy title, body and link columns, written empty since content moved",
            belongs: "dropped by the contract step; content is in org OLTP oxy_inbox.messages",
        },
    ),
    (
        "oidc_trust_policies",
        Control("which GitHub Actions runs may act as a service account"),
    ),
    (
        "oidc_trust_policy_grants",
        Control("what a run matching a trust policy is granted"),
    ),
    ("oidc_used_jti", Control("OIDC replay ledger")),
    (
        "oltp_branch_roles",
        Control("sealed per-branch passwords for org database roles"),
    ),
    (
        "oltp_branches",
        Control("org staging-branch registry: endpoint and sealed owner"),
    ),
    (
        "oltp_roles",
        Control("org database writer roles and sealed passwords"),
    ),
    ("oltp_tenants", Control("org database registry")),
    ("org_billing", Control("Stripe subscription state")),
    ("org_frontline_members", Control("frontline membership")),
    ("org_invitations", Control("org invitations")),
    (
        "org_kiosk_devices",
        Control("kiosk enrolment and secret hashes"),
    ),
    ("org_members", Control("org membership and role")),
    (
        "org_role_members",
        Control(
            "assignments reach is derived from. Judgement call — review with the operating graph",
        ),
    ),
    (
        "org_roles",
        Control("positions assignments point at. Judgement call — review with the operating graph"),
    ),
    ("org_secrets", Control("encrypted org secrets")),
    ("org_subdomains", Control("subdomain routing")),
    (
        "org_team_members",
        Control("team membership, an authz fact"),
    ),
    ("org_teams", Control("org teams")),
    (
        "org_token_policies",
        Control("what an org asks of the API tokens that reach it, read to authorize them"),
    ),
    ("organizations", Control("org registry")),
    (
        "partner_capabilities",
        Control("partner capability ceiling"),
    ),
    ("partner_grants", Control("partner grants")),
    ("partner_orgs", Control("partner to client mapping")),
    ("partner_publish_consent", Control("client publish consent")),
    ("partner_role_bindings", Control("partner operators")),
    ("quickbooks_oauth_states", Control("OAuth CSRF state")),
    ("reconcile_configs", Control("compiled reconcile.yml")),
    ("revisions", Control("compile revisions")),
    ("run_sequences", Control("run index counters")),
    (
        "schema_migration_definitions",
        Control("compiled schemas/*.sql"),
    ),
    ("seaql_migrations", Control("migration ledger")),
    ("seaql_migrations_airhouse", Control("migration ledger")),
    ("seaql_migrations_airway", Control("migration ledger")),
    ("seaql_migrations_analytics", Control("migration ledger")),
    ("seaql_migrations_cameras", Control("migration ledger")),
    ("seaql_migrations_oltp", Control("migration ledger")),
    ("seaql_migrations_orchestrator", Control("migration ledger")),
    ("seaql_migrations_workflow", Control("migration ledger")),
    ("secrets", Control("encrypted workspace secrets")),
    ("semantic_topics", Control("compiled .topic.yml")),
    ("semantic_views", Control("compiled .view.yml")),
    (
        "server_keys",
        Control(
            "secrets the deployment generates for itself: the session-signing root, the encryption-key check value, the KMS-sealed master key",
        ),
    ),
    (
        "service_accounts",
        Control("org-owned machine principals: identity and standing for API tokens"),
    ),
    ("settings", Control("legacy GitHub sync settings")),
    (
        "simulation_definitions",
        Control("compiled .simulation.yml"),
    ),
    (
        "simulation_run_fits",
        Backlog {
            holds: "fitted coefficients per edge and period",
            belongs: "Airhouse; a judgement call, since these are synthetic worlds",
        },
    ),
    (
        "simulation_run_periods",
        Backlog {
            holds: "per-period spend and profit",
            belongs: "Airhouse; a judgement call, since these are synthetic worlds",
        },
    ),
    (
        "simulation_runs",
        Control("simulation run lifecycle and spec snapshot"),
    ),
    ("sites", Control("camera fleet sites")),
    (
        "slack_channel_defaults",
        Control("Slack channel to workspace default"),
    ),
    ("slack_installations", Control("Slack bot installs")),
    ("slack_oauth_states", Control("OAuth state")),
    ("slack_seen_events", Control("Slack event dedupe ids")),
    (
        "slack_threads",
        Control("Slack thread to Oxy thread pointer"),
    ),
    (
        "slack_user_links",
        Control("Slack user to Oxy user identity"),
    ),
    (
        "slack_user_preferences",
        Control("default workspace and agent"),
    ),
    (
        "staff_notification_preferences",
        Control("which platform emails a staff member opted out of"),
    ),
    ("stripe_webhook_events", Control("Stripe webhook ledger")),
    (
        "tasks",
        Backlog {
            holds: "legacy title, question and answer",
            belongs: "org OLTP, or drop",
        },
    ),
    (
        "test_case_human_verdicts",
        Control("human pass or fail per eval case"),
    ),
    ("test_project_runs", Control("eval run grouping")),
    (
        "test_run_cases",
        Backlog {
            holds: "eval scores with prompt, expected and actual output, judge reasoning",
            belongs: "content → org OLTP; scores stay",
        },
    ),
    ("test_run_sequences", Control("eval run counters")),
    ("test_runs", Control("eval run header")),
    (
        "threads",
        Backlog {
            holds: "thread header with title, input, output and references",
            belongs: "content → org OLTP; the header stays",
        },
    ),
    (
        "unifi_credentials",
        Control("pointer to the UniFi key secret"),
    ),
    ("user_credentials", Control("login credentials")),
    ("users", Control("users")),
    ("verified_queries", Control("compiled verified .sql files")),
    (
        "work_items",
        Backlog {
            holds: "tasks, visits and checklists with title, body, due date and status",
            belongs: "org OLTP",
        },
    ),
    ("workspace_compiled_configs", Control("compiled config.yml")),
    (
        "workspace_factory_grants",
        Control(
            "which members an Oxy operator let into a workspace's Factory: ids and a timestamp",
        ),
    ),
    ("workspace_health_state", Control("workspace health")),
    (
        "workspace_health_transitions",
        Control("workspace health status changes, as dimension names and statuses"),
    ),
    ("workspace_members", Control("workspace membership")),
    (
        "workspace_compile_checks",
        Control("when each workspace's branch head is next compared with what it serves"),
    ),
    ("workspace_oxy_lockdown", Control("staff lockout")),
    (
        "workspace_preview_runs",
        Control("preview runs: which revision, key and target a staff preview run reads"),
    ),
    (
        "workspace_preview_schemas",
        Control("preview schema registry: names and expiry for the TTL drop"),
    ),
    (
        "workspace_preview_sources",
        Control("preview sandbox source overrides: var names and realm ids, never secrets"),
    ),
    (
        "workspace_preview_tables",
        Control("preview shadow map: which live tables a preview holds copies of"),
    ),
    (
        "workspace_previews",
        Control("which branches Oxy staff are previewing, and at which commit"),
    ),
    ("workspaces", Control("workspace registry")),
    ("world_model_configs", Control("compiled .world-model.yml")),
    (
        "world_model_events",
        Backlog {
            holds: "every org's POS orders and camera verdicts for the live feed, kept 6 hours",
            belongs: "workspace Airhouse; left until the Toast feed's future is decided",
        },
    ),
];

async fn live_tables() -> BTreeSet<String> {
    let (db, _url) = fresh_db(Schema::All).await;
    db.query_all_raw(Statement::from_string(
        DatabaseBackend::Postgres,
        "SELECT table_name FROM information_schema.tables \
         WHERE table_schema = 'public' AND table_type = 'BASE TABLE'",
    ))
    .await
    .expect("list tables")
    .into_iter()
    .map(|row| row.try_get::<String>("", "table_name").expect("table_name"))
    .collect()
}

fn listed() -> BTreeSet<&'static str> {
    TABLES.iter().map(|(name, _)| *name).collect()
}

#[tokio::test]
async fn every_table_in_oxys_database_has_a_placement() {
    let listed = listed();
    let unplaced: Vec<String> = live_tables()
        .await
        .into_iter()
        .filter(|table| !listed.contains(table.as_str()))
        .collect();
    assert!(
        unplaced.is_empty(),
        "tables with no placement in tests/platform/data_placement.rs: {unplaced:?}\n\n\
         Oxy's Postgres is the control plane. A table holding identity, access, \
         configuration, job bookkeeping, billing, audit or a signal goes in as \
         Control, with the reason. A table holding anything an org's systems or \
         people produced belongs in the org's data plane instead, picked by its \
         shape: facts and history in the workspace's Airhouse, records an app \
         edits in the org's OLTP database, files in app storage. See \
         internal-docs/data-placement.md."
    );
}

#[tokio::test]
async fn every_placement_names_a_table_that_exists() {
    let live = live_tables().await;
    let stale: Vec<&str> = listed()
        .into_iter()
        .filter(|table| !live.contains(*table))
        .collect();
    assert!(
        stale.is_empty(),
        "placements for tables no migrator creates any more: {stale:?} — delete \
         their rows. A backlog row removed this way is a table that left Oxy's \
         database."
    );
}

#[test]
fn no_table_is_placed_twice() {
    assert_eq!(
        listed().len(),
        TABLES.len(),
        "a table listed twice can be control plane and backlog at once"
    );
}

#[test]
fn every_placement_says_why() {
    for (table, placement) in TABLES {
        let reasons = match placement {
            Control(why) => vec![*why],
            Backlog { holds, belongs } => vec![*holds, *belongs],
        };
        assert!(
            reasons.iter().all(|reason| !reason.trim().is_empty()),
            "{table}: a placement with no reason is an exemption, not a decision"
        );
    }
}
