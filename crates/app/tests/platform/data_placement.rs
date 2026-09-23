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
    ("api_keys", Control("API key hashes")),
    ("app_admin_scope_orgs", Control("platform grant org scope")),
    ("app_admins", Control("platform staff grants")),
    (
        "app_builds",
        Control("custom-app build pointers; bundles live in S3"),
    ),
    ("app_definitions", Control("compiled .app.yml")),
    (
        "app_function_failure_alerts",
        Control("at-most-once failure page ledger"),
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
        "custom_app_event",
        Backlog {
            holds: "app SDK events with a free-form payload",
            belongs: "payload → Airhouse",
        },
    ),
    (
        "custom_app_migrations",
        Control("applied schema ledger per app"),
    ),
    (
        "custom_app_view_event",
        Control("Oxy's own app-open telemetry"),
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
        "metric_monitor_coverage",
        Control("scan coverage bookkeeping per segment"),
    ),
    ("monitor_configs", Control("compiled .monitor.yml")),
    (
        "notifications",
        Backlog {
            holds: "inbox title, body and link",
            belongs: "org OLTP; only the push signal stays",
        },
    ),
    ("oidc_used_jti", Control("OIDC replay ledger")),
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
    ("workspace_health_state", Control("workspace health")),
    ("workspace_members", Control("workspace membership")),
    ("workspace_oxy_lockdown", Control("staff lockout")),
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
