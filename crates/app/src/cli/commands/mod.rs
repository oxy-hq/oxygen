mod agentic_cli;
mod compile;
mod looker;
pub(crate) mod migrate;
mod migrate_agents;
mod migrate_automations;
/// `pub` so integration tests can drive the seed directly (see
/// `tests/seed_example_app.rs`) instead of shelling out to the binary — the
/// seed is a fixture the tests are built on, not just a CLI command.
pub mod oltp;
pub mod run;
pub mod seed;
mod seed_airhouse;
mod seed_compile;
mod seed_llm_keys;
mod seed_org_workspace;
pub(crate) mod seed_partners;
pub(crate) mod seed_platform_grants;
mod seed_threads;

use crate::cli::commands::migrate::migrate;
use crate::cli::commands::migrate_agents::{MigrateAgentsArgs, migrate_agents};
use crate::cli::commands::migrate_automations::{
    MOVED_EXIT_CODE, MigrateAutomationsArgs, moved_message,
};
use crate::cli::commands::run::{RunArgs, handle_run_command};
use crate::server::service::sync::{SyncFilter, sync_databases};
use ::oxy::adapters::secrets::SecretsManager;
use ::oxy::config::test_config::TestFileConfig;
use ::oxy::config::*;
use ::oxy::sentry_config;
use ::oxy::theme::StyledText;
use clap::Parser;
use model::{Automation, Config};
use oxy_shared::errors::OxyError;
use std::backtrace;
use std::error::Error;
use std::path::PathBuf;
use std::process::exit;

use ::oxy::config::WorkingCopy;
use tracing::debug;

// The one-shot subcommands `oxy` runs, in the order `--help` lists them after
// `oxy-server`'s own (`start`, `serve`, `worker`) and `oxy-api-eval`'s `test`.
// Registered with the `oxy-cli-shim` parser by `oxy-server` as an owner;
// `--version` is the shim's.
// A plain comment, not `///`: clap would print a doc comment on this enum as
// the root command's `about`, above `Usage:`.
#[derive(clap::Subcommand, Debug)]
pub enum SubCommand {
    /// Execute automation (.automation.yml or .procedure.yml) or SQL (.sql) files
    ///
    /// Run SQL queries against databases or execute automations for data processing.
    Run(RunArgs),
    /// Compile the workspace into the compile-boundary Postgres schema (Phase 1.6a observation mode).
    ///
    /// Walks every recognized YAML/SQL file, parses it, and writes a
    /// `revisions` row plus per-entity rows tagged with the new
    /// revision_id. Does NOT update `workspaces.current_revision_id`;
    /// runtime still reads YAML from disk in Phase 1.6a. Useful for
    /// inspecting what the compile boundary produces before any read
    /// path depends on it.
    Compile(compile::CompileArgs),
    /// Synchronize and collect metadata from connected databases
    ///
    /// Extract schema information, table structures, and relationships
    /// from your databases to enable better query suggestions and validation.
    Sync(SyncArgs),
    /// Validate configuration files for syntax and structure
    ///
    /// Check your config.yml, automation files, and agent configurations
    /// for errors and compliance with the expected schema.
    Validate(ValidateArgs),
    /// Migrate the database schema to the latest version
    Migrate,
    /// Moved to `oxyc migrate-automations` (prints the command and exits 2)
    ///
    /// Renaming legacy `.procedure.yml` / `.workflow.yml` files to
    /// `.automation.yml` is `oxyc migrate-automations [path] [--dry-run]` now
    /// (`npm i -g @oxy-hq/cli`). This stub stays for one release so an old
    /// invocation is told what to type instead.
    MigrateAutomations(MigrateAutomationsArgs),
    /// Translate classic `.agent.yml` agents to `.agentic.yml`
    ///
    /// Translates every classic agent whose fields map onto the agentic
    /// analytics config, deletes the classic file, and rewrites references to
    /// it (`agent_ref:`, paths, globs) across the project's YAML. A file with a
    /// field nothing agentic does (`omni_query`, `anonymize`, routing) is left
    /// in place and reported, unless `--force` drops the field. Never
    /// overwrites; collisions are reported. Run `--dry-run --json` first.
    MigrateAgents(MigrateAgentsArgs),

    /// Per-org OLTP Postgres: apply a workspace's schema.
    ///
    /// Status, provisioning, credentials, deprovisioning, the confinement
    /// audit and rotation are `oxyc oltp`.
    Oltp(crate::cli::commands::oltp::OltpArgs),

    /// Database seeding commands for development and testing
    #[clap(hide = true)]
    Seed(SeedArgs),
    /// Test Looker integrations
    ///
    /// Check that the Looker integrations configured in your project can connect.
    Looker(looker::LookerArgs),
    /// Run and debug agentic analytics pipelines
    ///
    /// Execute agentic pipelines with interactive debugging and event streaming.
    /// Supports analytics and builder domains. Use --json for LLM-readable output.
    /// Requires OXY_DATABASE_URL to be set.
    Agentic(agentic_cli::AgenticArgs),
}

#[derive(Parser, Debug)]
pub struct SyncArgs {
    /// Specific database to sync (syncs all if not specified)
    ///
    /// Target a single database connection from your config.yml
    /// instead of syncing metadata from all configured databases.
    database: Option<String>,
    /// Specific datasets/tables to sync within the database
    ///
    /// Limit synchronization to particular tables or schemas
    /// instead of processing the entire database structure.
    #[clap(long, short = 'd', num_args = 0..)]
    datasets: Vec<String>,
    /// Overwrite existing metadata files during sync
    ///
    /// Replace existing schema files and metadata instead of
    /// skipping tables that have already been synchronized.
    #[clap(
        long,
        short = 'o',
        default_value_t = false,
        help = "Overwrite existing files during sync"
    )]
    overwrite: bool,
}

#[derive(Parser, Debug)]
pub struct ValidateArgs {
    /// Validate a specific file instead of all configuration files
    ///
    /// Provide a path to an automation (.automation.yml) or agentic agent
    /// (.agentic.yml) file to validate just that file.
    ///
    /// Note: .agentic.yml validation is structural only — the file is parsed
    /// against the AgentConfig schema, but `databases:` entries and `llm.ref`
    /// are not resolved against config.yml, so a structurally valid file can
    /// still fail at runtime if those references don't exist.
    #[clap(long, short)]
    file: Option<std::path::PathBuf>,
}

#[derive(Parser, Debug)]
pub struct SeedArgs {
    /// Override the workspace path (defaults to `./examples`).
    #[clap(long)]
    pub workspace_path: Option<std::path::PathBuf>,
    /// Tear down instead of seeding: drops the demo workspace AND the seeded
    /// partner + tenant rows. Leaves the Local org + guest user in place.
    #[clap(long)]
    pub clear: bool,
    /// Skip compiling + promoting the seeded workspaces. Without the compile a
    /// seeded workspace answers `503 needs_recompile` until `oxy compile
    /// --promote` runs for it.
    #[clap(long, conflicts_with = "clear")]
    pub no_compile: bool,
    /// Copy the LLM keys each seeded workspace's `config.yml` references from
    /// this shell's environment into its secrets store. Opt-in because the
    /// values are your real credentials: a URL can look local and still reach a
    /// shared database (a port-forward), so the locality check alone is not
    /// consent. `just up` passes it; the database guard still applies.
    #[clap(long, conflicts_with = "clear")]
    pub llm_keys: bool,
    /// Also build the demo fixture app (`local/pulse-demo`): five real builds,
    /// a seven-day release history with the bad release live, seeded traffic
    /// and a few real runs. Local DB only; `just up` passes it.
    #[clap(long, conflicts_with = "clear")]
    pub fixtures: bool,
    /// Only give the existing org with this slug a local workspace (a Ready
    /// `Default` with a minimal `config.yml`, no repository) and print its id
    /// last. For a single-process dev or CI box: the product makes a workspace
    /// only from a GitHub import. Reuses the org's workspace when it has one.
    #[clap(long, conflicts_with_all = ["clear", "no_compile", "llm_keys", "fixtures"])]
    pub org_workspace: Option<String>,
}

/// Validates a single file based on its extension.
fn validate_single_file(file_path: &PathBuf, config: &Config) -> Result<(), String> {
    let file_name = file_path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    match () {
        _ if file_name.ends_with(".procedure.yml") || file_name.ends_with(".automation.yml") => {
            let automation = config.load_workflow(file_path).map_err(|e| e.to_string())?;
            config
                .validate_workflow(&automation)
                .map_err(|e| e.to_string())
        }
        _ if file_name.ends_with(".agentic.yml") => {
            agentic_analytics::config::AgentConfig::from_file(file_path)
                .map(|_| ())
                .map_err(|e| e.to_string())
        }
        _ if file_name.ends_with(".view.yml") || file_name.ends_with(".topic.yml") => {
            let parser_config = oxy_semantic::ParserConfig::new(
                file_path
                    .parent()
                    .and_then(|p| p.parent())
                    .unwrap_or(&config.workspace_path),
            );
            let parser = oxy_semantic::SemanticLayerParser::new(parser_config);
            if file_name.ends_with(".view.yml") {
                parser
                    .parse_view_file(file_path)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            } else {
                parser
                    .parse_topic_file(file_path)
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            }
        }
        _ => Err(format!(
            "Unknown file type: {}. Expected .automation.yml, .procedure.yml, .agentic.yml, .view.yml, or .topic.yml",
            file_path.display()
        )),
    }
}

/// Collect all `.view.yml` and `.topic.yml` files under `semantics/`.
fn list_semantic_files(project_path: &std::path::Path) -> Vec<PathBuf> {
    let semantics_dir = project_path.join("semantics");
    let mut files = Vec::new();
    for sub in &["views", "topics"] {
        let dir = semantics_dir.join(sub);
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str())
                    && (name.ends_with(".view.yml") || name.ends_with(".topic.yml"))
                {
                    files.push(path);
                }
            }
        }
    }
    files
}

/// The committed JSON Schemas, and the Rust types they are generated from.
///
/// ONE LIST, TWO READERS: `just gen-schemas` writes these files (the test below
/// in bless mode, `OXY_BLESS=1`), and `json_schemas_are_current` asserts the
/// committed copies still match. They
/// were regenerated BY HAND with nothing checking, which was tolerable while
/// the only consumer was an editor's `yaml-language-server` hint — a stale
/// schema there costs a missing autocomplete. It stopped being tolerable when
/// `oxyc validate` began validating real workspaces against these files: a
/// stale schema then rejects valid config, or accepts invalid config, and the
/// person who changed the Rust type has no way to know.
pub fn config_schemas() -> Vec<(&'static str, String)> {
    vec![
        (
            "config.json",
            serde_json::to_string_pretty(&schemars::schema_for!(Config)).unwrap_or_default(),
        ),
        (
            "workflow.json",
            serde_json::to_string_pretty(&schemars::schema_for!(Automation)).unwrap_or_default(),
        ),
        (
            "agentic.json",
            serde_json::to_string_pretty(&schemars::schema_for!(
                agentic_analytics::config::AgentConfig
            ))
            .unwrap_or_default(),
        ),
        (
            "agent-test.json",
            serde_json::to_string_pretty(&schemars::schema_for!(TestFileConfig))
                .unwrap_or_default(),
        ),
    ]
}

/// The process's panic hook. It **replaces** whatever is installed — Sentry's
/// panic integration, `human_panic`, and the hook `oxy_telemetry::stderr_capture`
/// sets at boot — rather than chaining: chaining would run Sentry's integration
/// beside the `capture_message` below and report every panic twice.
///
/// Because it replaces, it must write the panic itself. When the stderr capture
/// owns fd 2 (server commands in the JSON log format) that is one structured
/// `target: "panic"` line via `report_panic`; a multi-line `eprintln!` there would
/// arrive as one `ERROR` line plus one unstructured `INFO` line per backtrace
/// frame. Everywhere else — a terminal, a one-shot command — the plain print.
pub(crate) fn install_panic_hook() {
    std::panic::set_hook(Box::new(move |panic_info| {
        // Use eprintln! here — tracing macros must not be called inside a panic
        // hook because the current span's data may already be unwinding, causing
        // a second panic in tracing_subscriber's lookup_current.
        if !oxy_telemetry::stderr_capture::report_panic(panic_info) {
            let trace = backtrace::Backtrace::force_capture();
            eprintln!("panic occurred: {panic_info}\n{trace}");
        }

        // Capture panic in Sentry
        sentry::capture_message(
            &format!("Panic occurred: {}", panic_info),
            sentry::Level::Fatal,
        );
    }));
}

#[cfg(all(test, unix))]
mod panic_hook_tests {
    /// Child half of `the_shipped_hook_writes_one_structured_panic_line`: the
    /// boot order of a server process — stderr capture first, then the hook
    /// `prepare` installs — then a panic.
    #[test]
    fn panic_hook_child_body() {
        if std::env::var("OXY_PANIC_HOOK_CHILD").is_err() {
            return;
        }
        oxy_telemetry::stderr_capture::install(Some("oxy-test".into())).expect("installed");
        super::install_panic_hook();
        let _ = std::thread::spawn(|| panic!("boom after cli() replaced the hook")).join();
        oxy_telemetry::stderr_capture::finish(std::time::Duration::from_secs(5));
    }

    /// `prepare` replaces the panic hook after `logging::init` installed the
    /// capture's. The capture's own test calls `install` alone and so could not
    /// see that the shipped hook threw its structured line away; this one runs
    /// the real order in a child process and reads the child's stderr.
    #[test]
    fn the_shipped_hook_writes_one_structured_panic_line() {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cli::commands::panic_hook_tests::panic_hook_child_body",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("OXY_PANIC_HOOK_CHILD", "1")
            .output()
            .expect("child ran");
        let stderr = String::from_utf8_lossy(&out.stderr);
        let lines: Vec<serde_json::Value> = stderr
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                serde_json::from_str(l)
                    .unwrap_or_else(|e| panic!("non-JSON line on stderr ({e}): {l:?}\n{stderr}"))
            })
            .collect();
        let panics: Vec<_> = lines.iter().filter(|v| v["target"] == "panic").collect();
        assert_eq!(
            panics.len(),
            1,
            "exactly one structured panic line: {stderr}"
        );
        assert_eq!(
            panics[0]["panic.message"],
            "boom after cli() replaced the hook"
        );
        assert!(
            panics[0]["panic.backtrace"]
                .as_str()
                .is_some_and(|b| !b.is_empty())
        );
        assert!(
            !stderr.contains("panic occurred"),
            "the plain multi-line print must not also reach the capture: {stderr}"
        );
        assert_eq!(
            lines.len(),
            1,
            "no backtrace frame arrives as its own line: {stderr}"
        );
    }
}

/// What every command needs before it runs, called by the shim once argv is
/// parsed (`name` is the subcommand, `None` when there is none).
pub fn prepare(name: Option<&str>) {
    install_panic_hook();

    // Register the application's managed-connection resolver with oxy core
    // before any subcommand can build an `airhouse_managed` / `postgres_managed`
    // connector. Core resolves those through the port (it no longer depends on
    // `airhouse` / `oxy-oltp` directly); this is where oxy-app wires the impl.
    crate::managed_connection_resolver::install();

    // Add breadcrumb for CLI command
    if let Some(command_name) = name {
        sentry_config::add_breadcrumb(
            &format!("Executing CLI command: {}", command_name),
            "cli",
            sentry::Level::Info,
        );
        sentry_config::add_operation_context(command_name, None);
    }
}

/// Run one of [`SubCommand`].
pub async fn run(
    command: SubCommand,
    // What `oxy-server` composes from the surface crates. Of the commands here
    // only `migrate` (the post-migrate report) and `seed` (the example apps)
    // read it; the commands that mount routes are `oxy-server`'s own.
    seams: crate::server::router::SurfaceSeams,
) -> Result<(), Box<dyn Error>> {
    match command {
        SubCommand::Run(run_args) => {
            sentry_config::add_operation_context("run", Some(&run_args.file));
            handle_run_command(run_args).await?;
        }
        SubCommand::Compile(compile_args) => {
            sentry_config::add_operation_context("compile", None);
            if let Err(e) = compile::run_compile(compile_args).await {
                eprintln!("{}", format!("Compile failed: {e}").error());
                exit(1);
            }
        }
        SubCommand::Sync(sync_args) => {
            sentry_config::add_operation_context("sync", None);
            if let Some(ref db) = sync_args.database {
                sentry_config::add_database_context(db, None);
            }
            let config = ConfigBuilder::new()
                .with_workspace_path(&resolve_local_workspace_path()?)?
                .build_with_working_copy(Origin::Disk, OnMissing::Fail)
                .await?;

            let secrets_manager = SecretsManager::from_environment()?;
            let filter = sync_args.database.clone().map(|db| SyncFilter {
                database: Some(db),
                datasets: sync_args.datasets.clone(),
                tables: vec![],
            });
            debug!(sync_args = ?sync_args, "Syncing");
            println!("🔄Syncing databases");
            let sync_metrics =
                sync_databases(config.clone(), secrets_manager, filter, sync_args.overwrite)
                    .await?;
            println!(
                "✅Sync finished:\n\n{}",
                sync_metrics
                    .into_iter()
                    .map(|m| m.map_or_else(|e| e.to_string().error().to_string(), |v| v.to_string()))
                    .collect::<Vec<_>>()
                    .join("\n---\n")
            )
        }
        SubCommand::Validate(args) => {
            let config: ::oxy::config::ConfigManager<WorkingCopy> = ConfigBuilder::new()
                .with_workspace_path(&resolve_local_workspace_path()?)?
                .build_with_working_copy(Origin::Disk, OnMissing::Fail)
                .await?;

            if let Some(file_path) = args.file {
                let validation_result = validate_single_file(&file_path, config.get_config());
                match validation_result {
                    Ok(_) => println!("{}", format!("{} is valid", file_path.display()).success()),
                    Err(e) => {
                        println!("{}", e.error());
                        exit(1)
                    }
                }
            } else {
                // Validate all files, collecting all errors
                let cfg = config.get_config();
                let mut errors: Vec<String> = Vec::new();
                let mut valid_count = 0;

                for automation_file in cfg.list_workflows(&cfg.workspace_path) {
                    match validate_single_file(&automation_file, cfg) {
                        Ok(_) => valid_count += 1,
                        Err(e) => errors.push(format!("{}: {}", automation_file.display(), e)),
                    }
                }

                for agentic_file in cfg.list_agentic_agents(&cfg.workspace_path) {
                    match validate_single_file(&agentic_file, cfg) {
                        Ok(_) => valid_count += 1,
                        Err(e) => errors.push(format!("{}: {}", agentic_file.display(), e)),
                    }
                }

                // Validate semantic model files (.view.yml, .topic.yml)
                for semantic_file in list_semantic_files(&cfg.workspace_path) {
                    match validate_single_file(&semantic_file, cfg) {
                        Ok(_) => valid_count += 1,
                        Err(e) => errors.push(format!("{}: {}", semantic_file.display(), e)),
                    }
                }

                if errors.is_empty() {
                    println!(
                        "{}",
                        format!("All {} config files are valid", valid_count).success()
                    );
                } else {
                    for err in &errors {
                        println!("{}", err.error());
                    }
                    println!(
                        "{}",
                        format!(
                            "\n{} file(s) failed validation, {} file(s) valid",
                            errors.len(),
                            valid_count
                        )
                        .error()
                    );
                    exit(1)
                }
            }
        }
        SubCommand::Oltp(args) => {
            if let Err(e) = crate::cli::commands::oltp::oltp(args).await {
                eprintln!("{}", format!("{e}").error());
                exit(1);
            }
        }
        SubCommand::Migrate => {
            if let Err(e) = migrate().await {
                eprintln!("{}", format!("Migration failed: {e}").error());
                exit(1);
            }
            println!("{}", "Migration completed successfully".success());
            // `oxy migrate` is the chart's pre-upgrade hook, running the new
            // image before any pod rolls: report the custom apps this release
            // would stop. Report only; it never fails the hook.
            if let Some(report) = seams.post_migrate {
                eprintln!(
                    "{}",
                    report(crate::server::build_identity::build_label()).await
                );
            }
        }
        SubCommand::MigrateAutomations(args) => {
            eprintln!("{}", moved_message(&args).warning());
            exit(MOVED_EXIT_CODE);
        }
        SubCommand::MigrateAgents(args) => {
            if let Err(e) = migrate_agents(args) {
                eprintln!("{}", format!("Agent migration failed: {e}").error());
                exit(1);
            }
        }
        SubCommand::Seed(seed_args) => {
            handle_seed_command(seed_args, seams.example_apps).await?;
        }

        SubCommand::Looker(looker_args) => {
            looker::handle_looker_command(looker_args).await?;
        }
        SubCommand::Agentic(agentic_args) => {
            agentic_cli::handle_agentic_command(agentic_args).await?;
        }
    }

    Ok(())
}

async fn handle_seed_command(
    seed_args: SeedArgs,
    example_apps: Option<crate::server::router::ExampleAppsSeam>,
) -> Result<(), OxyError> {
    use seed::*;
    if let Some(slug) = seed_args.org_workspace.as_deref() {
        return seed_org_workspace::seed_org_workspace(slug)
            .await
            .map(|_| ());
    }
    if seed_args.clear {
        // Guard BOTH teardowns UP FRONT. clear_demo has no is_local() gate of its
        // own, so without this it would run (unguarded) before clear_partner_tenants
        // rejected a non-local DB — a partial, guard-bypassing teardown. Refuse
        // first; delete nothing on a remote DB.
        seed_partners::refuse_if_not_local()?;
        clear_demo(example_apps).await?;
        seed_partners::clear_partner_tenants().await
    } else {
        // Seeds the demo workspace AND (folded in) the partner + tenant data —
        // one command, no `--partners` flag — then compiles, and stores LLM keys
        // only when asked to (`--llm-keys`).
        let options = SeedOptions {
            compile: !seed_args.no_compile,
            llm_keys: seed_args.llm_keys,
            fixtures: seed_args.fixtures,
            airhouse: true,
        };
        seed_demo_with(seed_args.workspace_path, options, example_apps).await
    }
}

#[cfg(test)]
mod json_schema_freshness {
    use super::config_schemas;

    /// The committed `json-schemas/*.json` still match the Rust types.
    ///
    /// They were generated by hand (`oxy gen-config-schema`, since deleted),
    /// and nothing checked. That was survivable while the only consumer was an
    /// editor hint; `oxyc validate` now validates real workspaces against
    /// these files, so a stale one rejects valid config or accepts invalid
    /// config and the person who changed the Rust struct never finds out.
    ///
    /// **Bless mode.** With `OXY_BLESS=1` the test writes every generated
    /// schema over its committed copy instead of comparing, byte for byte what
    /// the deleted command wrote. `just gen-schemas` runs it that way.
    ///
    /// Reads the checkout, so it no-ops where the sources are not present —
    /// the same shape `route_catalog`'s tree walk uses.
    #[test]
    fn json_schemas_are_current() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../json-schemas");
        if !root.is_dir() {
            return;
        }
        if std::env::var_os("OXY_BLESS").is_some_and(|v| v == "1") {
            for (filename, generated) in config_schemas() {
                let path = root.join(filename);
                std::fs::write(&path, generated)
                    .unwrap_or_else(|e| panic!("could not write {}: {e}", path.display()));
            }
            return;
        }

        let mut stale = Vec::new();
        for (filename, generated) in config_schemas() {
            let path = root.join(filename);
            let committed = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => {
                    stale.push(format!("{filename} is missing"));
                    continue;
                }
            };
            // Compared as VALUES, not as text: `to_string_pretty` key order is
            // stable but whitespace at the end of a file is not worth failing
            // a build over.
            // Parsed with `?`-shaped handling rather than `unwrap_or_default`:
            // that turned two unparsable documents into `Null == Null` and
            // reported them as current.
            let a = match serde_json::from_str::<serde_json::Value>(&committed) {
                Ok(v) => v,
                Err(e) => {
                    stale.push(format!("{filename} is not valid JSON: {e}"));
                    continue;
                }
            };
            let b = match serde_json::from_str::<serde_json::Value>(&generated) {
                Ok(v) => v,
                Err(e) => {
                    stale.push(format!("{filename} could not be generated: {e}"));
                    continue;
                }
            };
            if a != b {
                stale.push(format!("{filename} differs from schema_for!()"));
            }
        }

        assert!(
            stale.is_empty(),
            "the committed JSON Schemas are out of date:\n  {}\n\n\
             Run `just gen-schemas` and commit the result. \
             `oxyc validate` validates real workspaces against these files, so a stale \
             one rejects valid config or accepts invalid config.",
            stale.join("\n  ")
        );
    }

    /// Schemas that are committed but NOT generated from a Rust type.
    ///
    /// Each is hand-maintained and has no `schema_for!()` to compare against,
    /// so the freshness check above cannot cover them. Listed by name so the
    /// exemption is a decision rather than an omission.
    const HAND_MAINTAINED: &[&str] = &[
        // The agentic browser-test harness's own format (web-app/tests/agentic).
        "flow-test.json",
        // The semantic layer's cross-workspace file; no single Rust type.
        "global-semantics.json",
    ];

    /// Every committed schema is either generated or explicitly exempt.
    ///
    /// THIS IS THE CHECK THAT WAS MISSING. `json_schemas_are_current` walks the
    /// GENERATED list and asks "is the committed copy current" — so a file that
    /// nothing generates is invisible to it. `automation.json` lived that way:
    /// committed, titled `Automation` like `workflow.json`, 6 KB behind it, and
    /// the web app's Monaco editor validated every `.automation.yml` against it.
    /// Two committed definitions of one Rust type, and the gate could not see
    /// the second one because it only ever looked at the first.
    #[test]
    fn every_committed_schema_is_generated_or_exempt() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../json-schemas");
        // A MISSING DIRECTORY IS A FAILURE, not a pass. Returning quietly here
        // would turn the whole gate into a no-op the moment `json-schemas/`
        // moved or was renamed — green, and blind to exactly the orphan it
        // exists to catch.
        let entries = std::fs::read_dir(&root).unwrap_or_else(|e| {
            panic!("could not read {}: {e}", root.display());
        });

        let generated: Vec<String> = config_schemas()
            .into_iter()
            .map(|(name, _)| name.to_string())
            .collect();

        let mut orphans = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if !name.ends_with(".json") {
                continue;
            }
            if generated.contains(&name) || HAND_MAINTAINED.contains(&name.as_str()) {
                continue;
            }
            orphans.push(name);
        }
        orphans.sort();

        assert!(
            orphans.is_empty(),
            "these schemas are committed but nothing generates them, and they are not \
             listed as hand-maintained: {orphans:?}\n\n\
             A schema nobody generates is a second definition of whatever it describes, \
             and it drifts silently — `automation.json` was 6 KB behind `workflow.json` \
             while the IDE validated against it. Either add it to `config_schemas()`, \
             add it to HAND_MAINTAINED with a reason, or delete it."
        );
    }
}
