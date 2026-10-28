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

use oxy::config::model::{Config, DatabaseType, SnowflakeAuthType};
use oxy_app_core::process_role::current_process_role;
use oxy_compile::{CompiledConfig, ConfigGate, merge_compiled_config};
use oxy_shared::fleet_role::Role;

/// The production config gate: merge the split columns and assert the result
/// deserialises into the runtime [`oxy::config::model::Config`], and that it
/// asks for no credential the pods serving it cannot have.
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
}

/// The environment variable that turns the file-credential refusal on. Without
/// it the gate only logs a warning for each such database, which is the rollout
/// gate (dev, then staging, then production): the refusal freezes a workspace's
/// served revision, and the in-product "create database connection" flow still
/// writes this shape, so it is switched on only once production has been checked
/// (the same pattern as `OXY_COMPILE_RECONCILE`).
pub const REFUSE_FILE_CREDENTIALS_ENV: &str = "OXY_COMPILE_REFUSE_FILE_CREDENTIALS";

/// Whether [`REFUSE_FILE_CREDENTIALS_ENV`] is set to `1`.
fn refusal_enabled() -> bool {
    std::env::var(REFUSE_FILE_CREDENTIALS_ENV).is_ok_and(|v| v == "1")
}

/// Refuse a database whose credential is a file in the repository, unless this
/// compile is single-process, and only when `refuse` is set (otherwise it
/// warns).
///
/// A serving pod has no working copy, so `key_path` (BigQuery) and
/// `private_key_path` (Snowflake) name a file that is not there. The compiler
/// also strips both values from the compiled config (`redact_inline_secrets`
/// blanks any key it takes for a secret, and `key_path` and `private_key` are
/// among them), so what a pod reads is an empty path whatever the file held.
/// The connection fails at the first query, on a pod, with an error about a
/// file the customer never wrote. Failing the compile puts the message where
/// the author is and keeps the previous revision serving.
///
/// `Role::All` is exempt: one process compiles and serves, from the directory
/// the file is in (the CLI, `oxy seed`, a laptop). Every other role compiles
/// for pods that are not that process.
fn reject_file_credentials(config: &Config, role: Role, refuse: bool) -> Result<(), String> {
    if role == Role::All {
        return Ok(());
    }
    for database in &config.databases {
        let refusal = match &database.database_type {
            // After the redaction above a file path reads back as an empty
            // path, still `Some`; only an absent key is `None`.
            DatabaseType::Bigquery(bigquery) if bigquery.key_path.is_some() => Some(format!(
                "database `{}`: `key_path` reads a service-account key file from the \
                 repository, which the pods that serve a compiled workspace do not have. A \
                 BigQuery key cannot be served from the hosted platform yet: `key_path_var` \
                 names a path, not the key, so it fails the same way on a pod. Contact Oxy \
                 support to connect this database",
                database.name
            )),
            DatabaseType::Snowflake(snowflake)
                if matches!(snowflake.auth_type, SnowflakeAuthType::PrivateKey { .. }) =>
            {
                Some(format!(
                    "database `{}`: `private_key_path` reads a private key file from the \
                     repository, which the pods that serve a compiled workspace do not have. \
                     Keep the credential in the secret manager: use `password_var`, or browser \
                     authentication where a person is present",
                    database.name
                ))
            }
            _ => None,
        };
        if let Some(message) = refusal {
            if refuse {
                return Err(message);
            }
            tracing::warn!(
                %message,
                "compile: file credential not refused; set {REFUSE_FILE_CREDENTIALS_ENV}=1 to fail the compile"
            );
        }
    }
    Ok(())
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

    const BIGQUERY_KEY_VAR: &str = r#"
models: []
databases:
  - name: warehouse
    type: bigquery
    key_path_var: BQ_KEY
    dataset: sales
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

    /// A file credential fails the compile on every role that compiles for
    /// other pods, and names the database and the key to use instead.
    #[test]
    fn a_file_credential_is_refused_when_the_compile_is_for_other_pods() {
        for role in [Role::Ide, Role::Worker, Role::Serve] {
            let err = reject_file_credentials(&config_of(BIGQUERY_KEY_FILE), role, true)
                .expect_err("a BigQuery key file is not servable");
            assert!(
                err.contains("`warehouse`") && err.contains("cannot be served"),
                "{err}"
            );

            let err = reject_file_credentials(&config_of(SNOWFLAKE_KEY_FILE), role, true)
                .expect_err("a Snowflake key file is not servable");
            assert!(
                err.contains("`wh`") && err.contains("password_var"),
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

    /// A single-process compile serves from the directory the file is in, and
    /// the secret-backed spellings are fine everywhere.
    #[test]
    fn the_single_process_role_and_secret_credentials_are_accepted() {
        assert!(reject_file_credentials(&config_of(BIGQUERY_KEY_FILE), Role::All, true).is_ok());
        assert!(reject_file_credentials(&config_of(SNOWFLAKE_KEY_FILE), Role::All, true).is_ok());
        for role in [Role::All, Role::Ide, Role::Worker, Role::Serve] {
            assert!(reject_file_credentials(&config_of(BIGQUERY_KEY_VAR), role, true).is_ok());
            assert!(
                reject_file_credentials(&config_of(SNOWFLAKE_PASSWORD_VAR), role, true).is_ok()
            );
            assert!(reject_file_credentials(&config_of(DUCKDB_LOCAL), role, true).is_ok());
        }
    }

    /// The rollout gate: without the refusal switched on the same configs only
    /// warn, so the change can be merged and deployed before production has been
    /// checked, and turned on per environment afterwards.
    #[test]
    fn without_the_switch_a_file_credential_only_warns() {
        for role in [Role::Ide, Role::Worker, Role::Serve] {
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
