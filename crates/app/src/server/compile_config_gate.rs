//! Compile-time read-deserialisation gate for `config.yml`.
//!
//! `oxy-compile` is platform-dep-free and can't see the strict `Config` type,
//! so it exposes the [`oxy_compile::ConfigGate`] hook and lets oxy-app supply
//! the check. [`RuntimeConfigGate`] runs the EXACT `from_value::<Config>` that
//! the request hot path runs (see
//! `workspace_context::try_attach_workspace_manager`), moved to compile time:
//! a config that won't deserialise becomes a compile failure (the revision is
//! marked `Failed` and never promoted, so the previous good revision keeps
//! serving) instead of a runtime fleet-wide 503.
//!
//! This is the strongest of the compile-boundary safety nets: it backstops ANY
//! compile transform — today secret redaction and the DuckDB→S3 mirror — not
//! just one known bug. Regression context: oxygen-internal#2520 (the
//! `s3_secret_type` discriminator was nulled by the redactor, so every ducklake
//! workspace failed to deserialise on the stateless fleet → hard 503).

use std::sync::Arc;

use oxy::config::model::{Config, DatabaseType, DuckDBOptions, SnowflakeAuthType};
use oxy_app_core::process_role::current_process_role;
use oxy_compile::{CompiledConfig, ConfigGate, merge_compiled_config};
use oxy_shared::fleet_role::Role;

use crate::server::feature_flags::rollout;

/// The production config gate: merge the split columns and assert the result
/// deserialises into the runtime [`oxy::config::model::Config`], and that it
/// asks for nothing the pods serving it cannot have.
pub struct RuntimeConfigGate;

impl ConfigGate for RuntimeConfigGate {
    fn check(&self, cfg: &CompiledConfig) -> Result<(), String> {
        let merged = merge_compiled_config(cfg);
        let config = serde_json::from_value::<Config>(merged).map_err(|e| {
            format!(
                "compiled config does not deserialise into the runtime Config — it would 503 \
                 the stateless fleet for this workspace; check for a corrupted enum \
                 discriminator or a redacted required field: {e}"
            )
        })?;
        reject_file_credentials(&config, current_process_role(), refusal_enabled())
    }

    /// Every role but the single-process one compiles for pods that are not
    /// the process compiling (see [`reject_file_credentials`]).
    fn compiles_for_cloud_roles(&self) -> bool {
        current_process_role() != Role::All
    }

    /// The `compile_refuse_retired_automation_steps` flag, or its deprecated
    /// override: `oxy-compile` cannot read flags, so the gate carries it.
    fn refuses_retired_automation_steps(&self) -> bool {
        rollout::is_on(&rollout::COMPILE_REFUSE_RETIRED_AUTOMATION_STEPS)
    }
}

/// The deprecated environment override of the `compile_refuse_file_credentials`
/// flag ([`rollout::COMPILE_REFUSE_FILE_CREDENTIALS`]), for one release.
pub use rollout::COMPILE_REFUSE_FILE_CREDENTIALS_ENV as REFUSE_FILE_CREDENTIALS_ENV;

/// Whether the file-credential refusal is on: the
/// `compile_refuse_file_credentials` flag, or `OXY_COMPILE_REFUSE_FILE_CREDENTIALS=1`
/// while that override lasts. Off, the gate only logs a warning for each such
/// database, which is the rollout (dev, then staging, then production): the
/// refusal freezes a workspace's served revision, and the in-product "create
/// database connection" flow still writes this shape, so it is switched on only
/// once production has been checked (the same pattern as `compile_reconcile`).
///
/// It governs the key files only. File DuckDB is refused on the cloud roles
/// whether it is on or not.
fn refusal_enabled() -> bool {
    rollout::is_on(&rollout::COMPILE_REFUSE_FILE_CREDENTIALS)
}

/// What a compile does with a database that reads a file from the repository.
///
/// **File DuckDB** (`type: duckdb` with `dataset:` or `path:`) is deprecated.
/// On every role but `OXY_ROLE=all` the compile fails, always, naming each such
/// database and pointing at Airhouse (`type: airhouse_managed`). The compiler
/// no longer copies its files to S3 on those roles
/// ([`ConfigGate::compiles_for_cloud_roles`]); the copies already there keep
/// serving the revisions compiled before this refusal until those workspaces
/// have moved. On `OXY_ROLE=all` it compiles with a deprecation warning.
///
/// **Key files.** A serving pod has no working copy, so `key_path` (BigQuery)
/// and `private_key_path` (Snowflake) name a file that is not there. The
/// compiler also strips both values from the compiled config
/// (`redact_inline_secrets` blanks any key it takes for a secret, and
/// `key_path` and `private_key` are among them), so what a pod reads is an
/// empty path whatever the file held. The connection fails at the first query,
/// on a pod, with an error about a file the customer never wrote. These warn,
/// and fail the compile only when `refuse` is set ([`refusal_enabled`]); the
/// replacement is the key's contents in
/// the secret store, `key_json_var` and `private_key_var`. BigQuery
/// `key_path_var` is deprecated with a warning: its value is a path, not the
/// key, so it fails on a pod the same way.
///
/// `Role::All` is exempt from the refusals: one process compiles and serves,
/// from the directory the files are in (the CLI, `oxy seed`, a laptop). Every
/// other role compiles for pods that are not that process.
fn reject_file_credentials(config: &Config, role: Role, refuse: bool) -> Result<(), String> {
    let file_duckdb = file_duckdb_databases(config);
    if role == Role::All {
        for name in &file_duckdb {
            tracing::warn!(
                database = %name,
                "compile: database `{name}` is a DuckDB file database (`dataset:` / `path:`), \
                 which is deprecated: a cloud deployment refuses it. Move its data to Airhouse \
                 (`type: airhouse_managed`)"
            );
        }
        return Ok(());
    }
    if !file_duckdb.is_empty() {
        return Err(file_duckdb_refusal(&file_duckdb));
    }
    for database in &config.databases {
        let name = &database.name;
        match &database.database_type {
            // The key's contents in a secret win over a key file at runtime,
            // so a database that has both is served.
            DatabaseType::Bigquery(bigquery) if bigquery.key_json_var.is_some() => {}
            // After the redaction above a file path reads back as an empty
            // path, still `Some`; only an absent key is `None`.
            DatabaseType::Bigquery(bigquery) if bigquery.key_path.is_some() => {
                file_credential(
                    refuse,
                    format!(
                        "database `{name}`: `key_path` reads a service-account key file from \
                         the repository, which the pods that serve a compiled workspace do not \
                         have. Put the key's contents in the workspace's secrets and name the \
                         secret with `key_json_var`"
                    ),
                )?;
            }
            DatabaseType::Bigquery(bigquery) if bigquery.key_path_var.is_some() => {
                tracing::warn!(
                    database = %name,
                    "compile: database `{name}`: `key_path_var` is deprecated — its secret holds \
                     a path to a key file, which the pods that serve a compiled workspace do not \
                     have. Put the key's contents in a secret and name it with `key_json_var`"
                );
            }
            DatabaseType::Snowflake(snowflake)
                if matches!(snowflake.auth_type, SnowflakeAuthType::PrivateKey { .. }) =>
            {
                file_credential(
                    refuse,
                    format!(
                        "database `{name}`: `private_key_path` reads a private key file from the \
                         repository, which the pods that serve a compiled workspace do not have. \
                         Put the key's contents (PEM) in the workspace's secrets and name the \
                         secret with `private_key_var`, or use `password_var`"
                    ),
                )?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// Refuse a key file when `refuse` is set; otherwise warn.
fn file_credential(refuse: bool, message: String) -> Result<(), String> {
    if refuse {
        return Err(message);
    }
    tracing::warn!(
        %message,
        "compile: file credential not refused; turn on the compile_refuse_file_credentials flag \
         (or set {REFUSE_FILE_CREDENTIALS_ENV}=1) to fail the compile"
    );
    Ok(())
}

/// The name of every DuckDB database whose data is a file or a directory in
/// the repository: `dataset:` or `path:` that is not a URL (`s3://…` is read
/// over the network). DuckLake keeps its data in S3 and its catalog in
/// Postgres, and is not one.
fn file_duckdb_databases(config: &Config) -> Vec<&str> {
    config
        .databases
        .iter()
        .filter(|database| match &database.database_type {
            DatabaseType::DuckDB(duckdb) => match &duckdb.options {
                DuckDBOptions::Local { file_search_path } => !file_search_path.contains("://"),
                DuckDBOptions::File { path } => !path.contains("://"),
                DuckDBOptions::DuckLake(_) => false,
            },
            _ => false,
        })
        .map(|database| database.name.as_str())
        .collect()
}

/// The compile failure for the file DuckDB databases of a cloud compile.
fn file_duckdb_refusal(names: &[&str]) -> String {
    let named = names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "database {named}: DuckDB file databases (`type: duckdb` with `dataset:` or `path:`) are \
         no longer supported in a cloud deployment — their data is files in the repository, \
         which the pods that serve a compiled workspace do not have. Move the data to the \
         workspace's Airhouse and declare the database as `type: airhouse_managed` under the \
         same name, so views, agents and apps that name it need no change. Airform models and \
         task exports cannot write into one either"
    )
}

/// The gate the production compile paths (worker + CLI) install on every
/// `CompileRequest`. A thin constructor so call sites don't import the type.
pub fn runtime_config_gate() -> Arc<dyn ConfigGate> {
    Arc::new(RuntimeConfigGate)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxy_compile::build_compiled_config;
    use serde_json::Value;

    // credential_chain ducklake — the exact dev-outage shape (#2520).
    const DUCKLAKE_CREDENTIAL_CHAIN: &str = r#"
models: []
databases:
  - name: lake
    type: duckdb
    schema_name: main
    data_path: s3://bucket/lake
    s3_secret_type: credential_chain
    chain: "sso;config"
    region: us-west-2
"#;

    // config-variant ducklake — exercises ManagedSecret env-var references
    // (`secret: AWS_S3_SECRET`), which must survive redaction.
    const DUCKLAKE_CONFIG_SECRET: &str = r#"
models: []
databases:
  - name: lake
    type: duckdb
    schema_name: main
    data_path: s3://bucket/lake
    s3_secret_type: config
    key_id: AKIAEXAMPLE
    secret: AWS_S3_SECRET
    region: us-west-2
    endpoint_url: https://s3.us-west-2.amazonaws.com
"#;

    const DUCKDB_LOCAL: &str = r#"
models: []
databases:
  - name: local
    type: duckdb
    dataset: data
"#;

    const MINIMAL: &str = r#"
models: []
databases: []
"#;

    /// The round-trip property: any `config.yml` that deserialises as authored
    /// MUST still deserialise after compile (split + redact + merge). This single
    /// assertion retroactively proves the whole gate — it fails on the #2520
    /// ducklake config if the `s3_secret_type` discriminator is ever corrupted
    /// again, before such a revision can be promoted.
    fn assert_roundtrips(yaml: &str) {
        // Fixture sanity: the YAML is a valid Config as authored.
        serde_yaml::from_str::<oxy::config::model::Config>(yaml)
            .unwrap_or_else(|e| panic!("fixture is not a valid Config: {e}\n{yaml}"));
        // Property under test: the compiled form still deserialises (== the gate).
        let value: Value = serde_yaml::from_str(yaml).unwrap();
        let cfg = build_compiled_config(value, None).expect("build_compiled_config");
        RuntimeConfigGate
            .check(&cfg)
            .unwrap_or_else(|e| panic!("compiled config must deserialise: {e}\n{yaml}"));
    }

    #[test]
    fn ducklake_credential_chain_roundtrips() {
        assert_roundtrips(DUCKLAKE_CREDENTIAL_CHAIN);
    }

    #[test]
    fn ducklake_config_secret_roundtrips() {
        assert_roundtrips(DUCKLAKE_CONFIG_SECRET);
    }

    #[test]
    fn duckdb_local_roundtrips() {
        assert_roundtrips(DUCKDB_LOCAL);
    }

    #[test]
    fn minimal_roundtrips() {
        assert_roundtrips(MINIMAL);
    }

    /// The gate must REJECT a config whose enum discriminator was nulled — the
    /// exact corruption the #2520 redactor caused. Proves the gate converts that
    /// outage into a compile failure instead of a runtime fleet 503.
    #[test]
    fn gate_rejects_nulled_s3_secret_type() {
        let value: Value = serde_yaml::from_str(DUCKLAKE_CREDENTIAL_CHAIN).unwrap();
        let mut cfg = build_compiled_config(value, None).unwrap();
        // Simulate the bug: null the discriminator in the databases column.
        if let Value::Array(dbs) = &mut cfg.databases
            && let Some(Value::Object(map)) = dbs.get_mut(0)
        {
            map.insert("s3_secret_type".into(), Value::Null);
        }
        assert!(
            RuntimeConfigGate.check(&cfg).is_err(),
            "a nulled discriminator must be rejected by the gate, not 503 the fleet"
        );
    }

    const BIGQUERY_KEY_FILE: &str = r#"
models: []
databases:
  - name: warehouse
    type: bigquery
    key_path: service-account.json
    dataset: sales
"#;

    const BIGQUERY_KEY_PATH_VAR: &str = r#"
models: []
databases:
  - name: warehouse
    type: bigquery
    key_path_var: BQ_KEY
    dataset: sales
"#;

    const BIGQUERY_KEY_JSON_VAR: &str = r#"
models: []
databases:
  - name: warehouse
    type: bigquery
    key_json_var: BQ_KEY_JSON
    dataset: sales
"#;

    /// The key's contents in a secret beside a leftover key file: the secret
    /// wins at runtime, so the file is not what is served.
    const BIGQUERY_KEY_JSON_VAR_AND_FILE: &str = r#"
models: []
databases:
  - name: warehouse
    type: bigquery
    key_json_var: BQ_KEY_JSON
    key_path: service-account.json
    dataset: sales
"#;

    const SNOWFLAKE_KEY_VAR: &str = r#"
models: []
databases:
  - name: wh
    type: snowflake
    account: acme
    username: svc
    warehouse: compute
    database: analytics
    private_key_var: SNOWFLAKE_PRIVATE_KEY
"#;

    const DUCKDB_FILE: &str = r#"
models: []
databases:
  - name: ledger
    type: duckdb
    path: data/ledger.duckdb
  - name: local
    type: duckdb
    dataset: .db
  - name: pg
    type: postgres
    host: db.example.com
    database: d
"#;

    const DUCKDB_REMOTE: &str = r#"
models: []
databases:
  - name: remote
    type: duckdb
    path: s3://bucket/ledger.duckdb
"#;

    const SNOWFLAKE_KEY_FILE: &str = r#"
models: []
databases:
  - name: wh
    type: snowflake
    account: acme
    username: svc
    warehouse: compute
    database: analytics
    private_key_path: keys/svc.p8
"#;

    const SNOWFLAKE_PASSWORD_VAR: &str = r#"
models: []
databases:
  - name: wh
    type: snowflake
    account: acme
    username: svc
    warehouse: compute
    database: analytics
    password_var: SNOWFLAKE_PASSWORD
"#;

    /// The compiled form of an authored `config.yml`, as the gate sees it: split
    /// into columns and redacted.
    fn compiled(yaml: &str) -> oxy_compile::CompiledConfig {
        let value: Value = serde_yaml::from_str(yaml).unwrap();
        build_compiled_config(value, None).expect("build_compiled_config")
    }

    fn config_of(yaml: &str) -> Config {
        serde_json::from_value(merge_compiled_config(&compiled(yaml))).expect("runtime Config")
    }

    const CLOUD_ROLES: [Role; 3] = [Role::Ide, Role::Worker, Role::Serve];

    /// A file credential fails the compile on every role that compiles for
    /// other pods, and names the database and the secret field to use instead.
    #[test]
    fn a_file_credential_is_refused_when_the_compile_is_for_other_pods() {
        for role in CLOUD_ROLES {
            let err = reject_file_credentials(&config_of(BIGQUERY_KEY_FILE), role, true)
                .expect_err("a BigQuery key file is not servable");
            assert!(
                err.contains("`warehouse`") && err.contains("key_json_var"),
                "{err}"
            );

            let err = reject_file_credentials(&config_of(SNOWFLAKE_KEY_FILE), role, true)
                .expect_err("a Snowflake key file is not servable");
            assert!(
                err.contains("`wh`") && err.contains("private_key_var"),
                "{err}"
            );
        }
    }

    /// The check works on what the gate is given — the REDACTED config — which
    /// is the one place a rename in the redactor would blind it. The path is
    /// blanked, not removed, so the field is still `Some`.
    #[test]
    fn the_refusal_survives_the_redaction_of_the_path() {
        let cfg = compiled(BIGQUERY_KEY_FILE);
        let databases = serde_json::to_string(&cfg.databases).unwrap();
        assert!(
            !databases.contains("service-account.json"),
            "the compiled config carries no key path: {databases}"
        );
        assert!(
            reject_file_credentials(&config_of(BIGQUERY_KEY_FILE), Role::Worker, true).is_err()
        );
    }

    /// The new secret fields survive the redaction (they end in `_var`) and
    /// are served on every role, with the refusal on.
    #[test]
    fn the_key_contents_in_a_secret_are_accepted_everywhere() {
        let cfg = compiled(BIGQUERY_KEY_JSON_VAR);
        assert!(
            serde_json::to_string(&cfg.databases)
                .unwrap()
                .contains("BQ_KEY_JSON")
        );
        let cfg = compiled(SNOWFLAKE_KEY_VAR);
        assert!(
            serde_json::to_string(&cfg.databases)
                .unwrap()
                .contains("SNOWFLAKE_PRIVATE_KEY")
        );
        for role in [Role::All, Role::Ide, Role::Worker, Role::Serve] {
            for yaml in [
                BIGQUERY_KEY_JSON_VAR,
                BIGQUERY_KEY_JSON_VAR_AND_FILE,
                SNOWFLAKE_KEY_VAR,
                SNOWFLAKE_PASSWORD_VAR,
            ] {
                assert!(
                    reject_file_credentials(&config_of(yaml), role, true).is_ok(),
                    "{role:?}: {yaml}"
                );
            }
        }
    }

    /// `key_path_var` is deprecated with a warning, never refused: its value is
    /// a path, so it fails on a pod as a key file does, but it was the
    /// documented secret spelling until `key_json_var` existed.
    #[test]
    fn key_path_var_only_warns() {
        for role in CLOUD_ROLES {
            assert!(reject_file_credentials(&config_of(BIGQUERY_KEY_PATH_VAR), role, true).is_ok());
        }
    }

    /// File DuckDB is refused on every cloud role whether the switch is on or
    /// not, in one message naming each such database and pointing at Airhouse.
    #[test]
    fn file_duckdb_is_refused_on_a_cloud_role_regardless_of_the_switch() {
        for role in CLOUD_ROLES {
            for refuse in [false, true] {
                for yaml in [DUCKDB_FILE, DUCKDB_LOCAL] {
                    let err = reject_file_credentials(&config_of(yaml), role, refuse)
                        .expect_err("file DuckDB is not served in the cloud");
                    assert!(err.contains("airhouse_managed"), "{err}");
                }
                let err =
                    reject_file_credentials(&config_of(DUCKDB_FILE), role, refuse).unwrap_err();
                assert!(
                    err.contains("`ledger`") && err.contains("`local`") && !err.contains("`pg`"),
                    "{err}"
                );
            }
        }
    }

    /// A DuckDB whose path is a URL reads over the network, not from the
    /// repository, and a DuckLake keeps its data in S3: neither is refused.
    #[test]
    fn a_remote_duckdb_and_a_ducklake_are_not_file_databases() {
        for role in CLOUD_ROLES {
            assert!(reject_file_credentials(&config_of(DUCKDB_REMOTE), role, false).is_ok());
            assert!(
                reject_file_credentials(&config_of(DUCKLAKE_CREDENTIAL_CHAIN), role, false).is_ok()
            );
        }
    }

    /// Local `oxy start` (`OXY_ROLE=all`) is unchanged: a file DuckDB and the
    /// key files compile, with a deprecation warning for the DuckDB.
    #[test]
    fn the_single_process_role_compiles_everything_it_did() {
        for yaml in [
            BIGQUERY_KEY_FILE,
            BIGQUERY_KEY_PATH_VAR,
            SNOWFLAKE_KEY_FILE,
            DUCKDB_FILE,
            DUCKDB_LOCAL,
        ] {
            assert!(
                reject_file_credentials(&config_of(yaml), Role::All, true).is_ok(),
                "{yaml}"
            );
        }
        for role in [Role::All, Role::Ide, Role::Worker, Role::Serve] {
            assert!(
                reject_file_credentials(&config_of(SNOWFLAKE_PASSWORD_VAR), role, true).is_ok()
            );
        }
    }

    /// The rollout gate: without the refusal switched on the key files only
    /// warn, so the change can be merged and deployed before production has
    /// been checked, and turned on per environment afterwards.
    #[test]
    fn without_the_switch_a_file_credential_only_warns() {
        for role in CLOUD_ROLES {
            assert!(
                reject_file_credentials(&config_of(BIGQUERY_KEY_FILE), role, false).is_ok(),
                "{role:?}"
            );
            assert!(reject_file_credentials(&config_of(SNOWFLAKE_KEY_FILE), role, false).is_ok());
        }
        assert!(
            reject_file_credentials(&config_of(BIGQUERY_KEY_FILE), Role::Worker, true).is_err()
        );
    }

    /// The gate tells the compiler whether it compiles for the cloud roles; a
    /// test binary has no role set, which is the single-process role, so the
    /// compiler keeps copying file DuckDB and the automation writes only warn.
    #[test]
    fn the_gate_reports_a_single_process_compile() {
        assert!(!RuntimeConfigGate.compiles_for_cloud_roles());
    }

    /// Through the real gate, on the process role it is running under: a test
    /// binary has none set, which is the single-process role.
    #[test]
    fn the_gate_applies_the_process_role() {
        assert_eq!(current_process_role(), Role::All);
        RuntimeConfigGate
            .check(&compiled(BIGQUERY_KEY_FILE))
            .expect("single-process compiles keep working");
    }

    /// A genuinely broken config is still rejected — the gate is a faithful
    /// stand-in for the runtime parse.
    ///
    /// This used to prove that with an omitted `models` key, which stopped
    /// being broken when `models` and `databases` gained `#[serde(default)]`:
    /// a workspace that declares neither is valid now, and the gate is right to
    /// accept it (pinned in the test below). The property here is unchanged —
    /// a config the runtime cannot parse must fail the compile rather than 503
    /// the fleet — so it needs an input that is still unparseable, and a
    /// `models` that is not a sequence is one.
    #[test]
    fn gate_rejects_structurally_invalid_config() {
        let cfg =
            build_compiled_config(serde_json::json!({ "models": "not-a-sequence" }), None).unwrap();
        assert!(
            RuntimeConfigGate.check(&cfg).is_err(),
            "a `models` that is not a sequence must fail the compile, not 503 the fleet"
        );
    }

    /// The other half of that change: a `config.yml` declaring neither `models`
    /// nor `databases` now parses, so the gate must NOT reject it.
    ///
    /// Worth its own test because the failure it guards is silent in both
    /// directions — without it the serde defaults could be reverted and every
    /// remaining test would still pass, while the workspaces this PR unbroke
    /// would go back to compiling into an empty config.
    #[test]
    fn gate_accepts_a_config_declaring_neither_models_nor_databases() {
        let cfg = build_compiled_config(serde_json::json!({}), None).unwrap();
        RuntimeConfigGate
            .check(&cfg)
            .expect("a config with nothing to declare must compile");
    }
}
