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

use oxy_compile::{CompiledConfig, ConfigGate, merge_compiled_config};

/// The production config gate: merge the split columns and assert the result
/// deserialises into the runtime [`oxy::config::model::Config`].
pub struct RuntimeConfigGate;

impl ConfigGate for RuntimeConfigGate {
    fn check(&self, cfg: &CompiledConfig) -> Result<(), String> {
        let merged = merge_compiled_config(cfg);
        serde_json::from_value::<oxy::config::model::Config>(merged)
            .map(|_| ())
            .map_err(|e| {
                format!(
                    "compiled config does not deserialise into the runtime Config — it would 503 \
                     the stateless fleet for this workspace; check for a corrupted enum \
                     discriminator or a redacted required field: {e}"
                )
            })
    }
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
