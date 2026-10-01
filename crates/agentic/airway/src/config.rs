//! YAML schema for `.airway.yml` pipeline specs.
//!
//! Source and destination configs are intentionally **open-ended**:
//! each carries a `kind` discriminator plus a free-form `config`
//! payload. The factories ([`crate::source_factory`],
//! [`crate::destination_factory`]) dispatch `kind` to the matching
//! airway connector constructor. Adding a new connector means adding
//! one dispatch arm — the YAML schema doesn't change.

use serde::{Deserialize, Serialize};

/// Parsed `.airway.yml` document.
///
/// Strict (`deny_unknown_fields`) so YAML schema drift surfaces as a
/// parse error rather than silently ignored config.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AirwayPipelineSpec {
    /// Pipeline name. Used as the `pipeline_name` on every emitted event
    /// and as the primary key in the per-pipeline state store row.
    pub name: String,

    /// Human-readable description for agent / UI consumption. Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    pub source: SourceConfig,

    /// Destination configuration. In authored YAML this is always a
    /// [`DestinationSpec::Reference`] to a `config.yml` database; the
    /// `agentic-pipeline` executor resolves it into a
    /// [`DestinationSpec::Inline`] (credentialed `kind` + `config`)
    /// before the worker runs. `Inline` is also accepted directly so
    /// the `memory` test fixture and post-resolution specs parse.
    pub destination: DestinationSpec,

    /// Optional explicit subset of resources to extract. When omitted,
    /// every resource the source advertises is extracted. The names
    /// must match the source's `resources()` output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resources: Vec<String>,

    /// Allow more than one run of this pipeline at a time.
    ///
    /// **Defaults to `false` (single-flight enforced), and that is almost
    /// always what you want.** Two concurrent runs of one pipeline are
    /// incorrect on two independent axes: they read-modify-write a single
    /// `airway_pipeline_state` cursor row (keyed by `pipeline_name`), so one
    /// silently loses its window; and their end-of-load folds can overlap
    /// snapshots, so a row changed between the two runs lands twice.
    ///
    /// Set `true` only for a pipeline that owns no incremental cursor and
    /// writes no `replacing` table — a full-refresh append into a
    /// partitioned target, say. If you are unsure, leave it off: the failure
    /// it prevents is silent, and the cost of the guard is a skipped
    /// scheduler tick.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_concurrent_runs: bool,

    /// Maximum in-flight extractions when running resources in
    /// parallel. Capped at [`MAX_CONCURRENCY`] to avoid DoS-ing the
    /// upstream source. Defaults to 1 (sequential, matches airway's
    /// `Pipeline::run_source` semantics).
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,

    /// Airway's streaming extract↔sink pipeline (concurrent, per-table
    /// progress). **Default on** — only takes effect when the
    /// destination is streaming-capable (Airhouse, Postgres-wire);
    /// otherwise the bulk path runs transparently. Set `false` to force
    /// the bulk path.
    #[serde(default = "default_streaming")]
    pub streaming: bool,

    /// Bounded extract→sink channel capacity on the streaming path.
    /// `None` → `2 * concurrency`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_capacity: Option<usize>,
}

/// Hard ceiling for the YAML `concurrency:` field. Higher values are
/// rejected at validation time.
pub const MAX_CONCURRENCY: usize = 16;

/// Pipeline-name prefix reserved for workspace previews.
///
/// A preview's Airway sample runs under `preview:<key>:<name>`, and that name
/// keys its own lease, cursor, stored-schema and load-audit rows — which is
/// what keeps production's `(workspace_id, name)` rows untouched. The
/// isolation holds only if no authored pipeline can already be called that:
/// a YAML `name: preview:…` would share a preview's rows, and the legacy
/// `airway_pipeline_state` adoption (keyed by name alone) could hand a sample
/// production's cursor. So [`AirwayPipelineSpec::validate`] refuses the
/// prefix from every caller that parses authored YAML.
pub const RESERVED_NAME_PREFIX: &str = "preview:";

fn default_concurrency() -> usize {
    1
}

fn default_streaming() -> bool {
    true
}

/// Source connector config. `kind` selects the airway connector
/// module; `config` carries the connector-specific params.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceConfig {
    /// Connector kind (`rest_api`, `filesystem`, `postgres_cdc`,
    /// `sql_database`, or any of the verified vendor connectors —
    /// see `crate::source_factory::build_source_connector` for the
    /// currently-wired list).
    pub kind: String,

    /// Connector-specific configuration. Re-deserialized into the
    /// matching `*Config` struct by the factory. JSON-shaped so we
    /// don't pull airway's individual config types into this surface.
    #[serde(default)]
    pub config: serde_json::Value,
}

/// Destination, in one of two shapes:
///
/// * [`DestinationSpec::Reference`] — what users author: a pointer to a
///   `config.yml` database by name. Credentials never live in the
///   `.airway.yml`. The `agentic-pipeline` executor resolves the named
///   database (including per-subject `airhouse_managed` minting) into a
///   concrete connection string at run time.
/// * [`DestinationSpec::Inline`] — a resolved/literal connector
///   (`kind` + `config`). Produced by the executor from a `Reference`,
///   and accepted directly for the `memory` test fixture.
///
/// Untagged: a mapping with a `database:` key parses as `Reference`,
/// anything else as `Inline`. `Reference` is tried first.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum DestinationSpec {
    Reference(DestinationRef),
    Inline(DestinationConfig),
}

/// A `config.yml` database reference. The airway destination `kind` is
/// derived from the database's configured type by the host resolver.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationRef {
    /// Name of a database defined in the project's `config.yml`.
    pub database: String,

    /// Logical dataset/schema the pipeline writes into.
    pub dataset_name: String,

    /// Optional table-name separator (e.g. `___`). When set on an
    /// airhouse destination, a flat source table named `<schema><sep><table>`
    /// is written to schema `<schema>` instead of the single `dataset_name`
    /// root — useful for ClickHouse sources that flatten schema into the
    /// table name. Omit for the default single-root behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_separator: Option<String>,
}

/// A concrete destination connector. Either the resolved form of a
/// [`DestinationRef`] or a literal (used by the `memory` fixture).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DestinationConfig {
    /// Destination kind (`airhouse`, `memory`, `postgres`, etc. — see
    /// `crate::destination_factory::build_destination` for the
    /// currently-wired list).
    pub kind: String,

    /// Destination-specific configuration.
    #[serde(default)]
    pub config: serde_json::Value,
}

impl DestinationSpec {
    /// The concrete connector the worker needs. Errors if the
    /// destination is still an unresolved [`DestinationRef`] — the
    /// `agentic-pipeline` executor is responsible for resolving it
    /// before the worker runs.
    pub fn as_inline(&self) -> Result<&DestinationConfig, crate::AirwayError> {
        match self {
            DestinationSpec::Inline(c) => Ok(c),
            DestinationSpec::Reference(r) => Err(crate::AirwayError::Other(format!(
                "airway destination `database: {}` was not resolved before run \
                 (this is an agentic-pipeline executor bug)",
                r.database
            ))),
        }
    }

    /// The referenced `config.yml` database name, if this is a
    /// [`DestinationRef`]. `None` for inline connectors.
    pub fn database_ref(&self) -> Option<&str> {
        match self {
            DestinationSpec::Reference(r) => Some(&r.database),
            DestinationSpec::Inline(_) => None,
        }
    }
}

impl AirwayPipelineSpec {
    /// Parse a `.airway.yml` document with no variable substitution.
    pub fn from_yaml_str(yaml: &str) -> Result<Self, crate::AirwayError> {
        let spec: AirwayPipelineSpec = serde_yaml::from_str(yaml)
            .map_err(|e| crate::AirwayError::Other(format!("invalid .airway.yml: {e}")))?;
        spec.validate()?;
        Ok(spec)
    }

    /// Render the YAML through minijinja with `variables` as context,
    /// then parse.
    ///
    /// `None` (or an empty object) skips rendering entirely so a spec
    /// with literal `{{ }}` it doesn't intend as a template isn't
    /// disturbed. Rendering uses **strict** undefined behaviour: a
    /// `{{ missing }}` reference with no matching variable is a hard
    /// error rather than silently expanding to an empty string — a
    /// silently-empty `base_url:` / `connection_string:` would produce
    /// a broken pipeline that fails much later and more obscurely.
    ///
    /// Variables must be a JSON object; scalars/arrays at the top level
    /// are rejected (there'd be no key to reference them by).
    pub fn from_yaml_with_vars(
        yaml: &str,
        variables: Option<&serde_json::Value>,
    ) -> Result<Self, crate::AirwayError> {
        let ctx = match variables {
            None => None,
            Some(serde_json::Value::Object(m)) if m.is_empty() => None,
            Some(v @ serde_json::Value::Object(_)) => Some(v),
            Some(_) => {
                return Err(crate::AirwayError::Other(
                    "airway `variables` must be a mapping/object".into(),
                ));
            }
        };

        let Some(ctx) = ctx else {
            return Self::from_yaml_str(yaml);
        };

        let mut env = minijinja::Environment::new();
        env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
        let rendered = env
            .render_str(yaml, minijinja::Value::from_serialize(ctx))
            .map_err(|e| {
                crate::AirwayError::Other(format!("airway variable substitution failed: {e}"))
            })?;
        Self::from_yaml_str(&rendered)
    }

    /// Validate invariants the type system doesn't capture.
    pub fn validate(&self) -> Result<(), crate::AirwayError> {
        if self.name.trim().is_empty() {
            return Err(crate::AirwayError::Other(
                "airway pipeline `name` must not be empty".into(),
            ));
        }
        if self.name.starts_with(RESERVED_NAME_PREFIX) {
            return Err(crate::AirwayError::Other(format!(
                "airway pipeline `name` must not start with `{RESERVED_NAME_PREFIX}`: \
                 that prefix is reserved for workspace previews"
            )));
        }
        if self.source.kind == "quickbooks"
            && self.source.config.get("client_id").is_some()
            && self.source.config.get("client_id_var").is_some()
        {
            return Err(crate::AirwayError::Other(
                "quickbooks config: name the client id once — `client_id` or `client_id_var`, \
                 not both"
                    .into(),
            ));
        }
        if self.concurrency == 0 {
            return Err(crate::AirwayError::Other(
                "`concurrency` must be >= 1".into(),
            ));
        }
        if self.concurrency > MAX_CONCURRENCY {
            return Err(crate::AirwayError::Other(format!(
                "`concurrency` is {} but capped at {MAX_CONCURRENCY}",
                self.concurrency
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_rest_api_spec() {
        let yaml = r#"
name: shopify_orders
source:
  kind: rest_api
  config:
    base_url: https://api.example.com
    endpoints: []
destination:
  kind: memory
  config:
    dataset_name: scratch
"#;
        let spec = AirwayPipelineSpec::from_yaml_str(yaml).expect("parse");
        assert_eq!(spec.name, "shopify_orders");
        assert_eq!(spec.source.kind, "rest_api");
        assert_eq!(spec.destination.as_inline().expect("inline").kind, "memory");
        assert_eq!(spec.concurrency, 1);
        assert!(spec.resources.is_empty());
    }

    #[test]
    fn parses_database_reference_destination() {
        let yaml = r#"
name: shopify_raw
source:
  kind: rest_api
  config:
    base_url: https://api.example.com
    endpoints: []
destination:
  database: my_warehouse
  dataset_name: shopify_raw
"#;
        let spec = AirwayPipelineSpec::from_yaml_str(yaml).expect("parse");
        assert_eq!(spec.destination.database_ref(), Some("my_warehouse"));
        match &spec.destination {
            DestinationSpec::Reference(r) => {
                assert_eq!(r.database, "my_warehouse");
                assert_eq!(r.dataset_name, "shopify_raw");
            }
            other => panic!("expected Reference, got {other:?}"),
        }
        // The worker must refuse an unresolved reference.
        let err = spec.destination.as_inline().unwrap_err();
        assert!(err.to_string().contains("my_warehouse"), "got: {err}");
    }

    #[test]
    fn rejects_reference_with_unknown_field() {
        let yaml = r#"
name: x
source:
  kind: memory
destination:
  database: w
  dataset_name: d
  connection_string: sneaky
"#;
        // `connection_string` is not a `DestinationRef` field and the
        // mapping has no `kind`, so neither untagged variant matches.
        let err = AirwayPipelineSpec::from_yaml_str(yaml).unwrap_err();
        assert!(
            err.to_string().contains("invalid .airway.yml"),
            "got: {err}"
        );
    }

    #[test]
    fn parses_full_spec_with_resources_and_concurrency() {
        let yaml = r#"
name: cdc_users
description: Postgres CDC into airhouse
source:
  kind: postgres_cdc
  config:
    connection_string: postgres://user:pass@host/db
    slot_name: oxy_users
    publication_name: oxy_pub
destination:
  kind: airhouse
  config:
    connection_string: postgres://user:pass@host/raw
    dataset_name: shopify_raw
resources:
  - users
  - orders
concurrency: 4
"#;
        let spec = AirwayPipelineSpec::from_yaml_str(yaml).expect("parse");
        assert_eq!(spec.concurrency, 4);
        assert_eq!(spec.resources, vec!["users", "orders"]);
        assert_eq!(
            spec.description.as_deref(),
            Some("Postgres CDC into airhouse")
        );
    }

    #[test]
    fn rejects_empty_name() {
        let yaml = r#"
name: ""
source:
  kind: memory
destination:
  kind: memory
"#;
        let err = AirwayPipelineSpec::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("name"));
    }

    /// A QuickBooks client id comes from one place: the literal, or the secret
    /// `client_id_var` resolves (a preview sandbox's development keys).
    #[test]
    fn a_quickbooks_client_id_is_named_once() {
        let yaml = |extra: &str| {
            format!(
                "name: qb\nsource:\n  kind: quickbooks\n  config:\n    realm_id: \"1\"\n{extra}\
                 destination:\n  database: airhouse\n  dataset_name: qb\n"
            )
        };
        let both = "    client_id: c\n    client_id_var: QB_CLIENT_ID\n";
        let err = AirwayPipelineSpec::from_yaml_str(&yaml(both)).unwrap_err();
        assert!(err.to_string().contains("not both"), "{err}");
        assert!(AirwayPipelineSpec::from_yaml_str(&yaml("    client_id: c\n")).is_ok());
        assert!(AirwayPipelineSpec::from_yaml_str(&yaml("    client_id_var: V\n")).is_ok());
    }

    /// `preview:` names key a preview's own lease, cursor and audit rows, so an
    /// authored pipeline may not take one — through the plain parse, through
    /// templating (a variable could render the prefix in), or through a spec a
    /// caller deserialized itself and then validated, as the compiled-row
    /// readers do.
    #[test]
    fn a_pipeline_name_cannot_start_with_preview() {
        let yaml = r#"
name: "preview:feat_je_v2_91ab0e:quickbooks_financials"
source:
  kind: memory
destination:
  kind: memory
"#;
        let err = AirwayPipelineSpec::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("reserved"), "got: {err}");

        let vars = serde_json::json!({ "env": "preview:x" });
        let templated =
            "name: \"{{ env }}\"\nsource:\n  kind: memory\ndestination:\n  kind: memory\n";
        let err = AirwayPipelineSpec::from_yaml_with_vars(templated, Some(&vars)).unwrap_err();
        assert!(err.to_string().contains("reserved"), "got: {err}");

        let mut spec = AirwayPipelineSpec::from_yaml_str(
            "name: quickbooks_financials\nsource:\n  kind: memory\ndestination:\n  kind: memory\n",
        )
        .expect("a plain name parses");
        spec.name = format!("{RESERVED_NAME_PREFIX}abc_123456:quickbooks_financials");
        assert!(spec.validate().is_err(), "validate() itself refuses it");

        // Only the exact prefix is reserved: a name merely containing the word
        // is an ordinary pipeline.
        for ok in ["previews_daily", "quickbooks_preview", "preview_rollups"] {
            let spec = AirwayPipelineSpec::from_yaml_str(&format!(
                "name: {ok}\nsource:\n  kind: memory\ndestination:\n  kind: memory\n"
            ));
            assert!(spec.is_ok(), "`{ok}` must stay valid: {:?}", spec.err());
        }
    }

    #[test]
    fn rejects_concurrency_zero() {
        let yaml = r#"
name: x
source:
  kind: memory
destination:
  kind: memory
concurrency: 0
"#;
        let err = AirwayPipelineSpec::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("concurrency"));
    }

    #[test]
    fn rejects_concurrency_above_cap() {
        let yaml = format!(
            r#"
name: x
source:
  kind: memory
destination:
  kind: memory
concurrency: {}
"#,
            MAX_CONCURRENCY + 1
        );
        let err = AirwayPipelineSpec::from_yaml_str(&yaml).unwrap_err();
        assert!(err.to_string().contains("capped"));
    }

    #[test]
    fn rejects_unknown_top_level_field() {
        let yaml = r#"
name: x
source:
  kind: memory
destination:
  kind: memory
total_chaos: true
"#;
        let err = AirwayPipelineSpec::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("invalid .airway.yml"));
    }

    // ── Variable templating ──────────────────────────────────────────────

    const TEMPLATED: &str = r#"
name: shopify_{{ env }}
source:
  kind: rest_api
  config:
    base_url: "{{ base }}"
    endpoints: []
destination:
  kind: memory
  config:
    dataset_name: scratch
"#;

    #[test]
    fn renders_variables() {
        let vars = serde_json::json!({ "env": "prod", "base": "https://api.example.com" });
        let spec = AirwayPipelineSpec::from_yaml_with_vars(TEMPLATED, Some(&vars)).expect("render");
        assert_eq!(spec.name, "shopify_prod");
        assert_eq!(spec.source.config["base_url"], "https://api.example.com");
    }

    #[test]
    fn none_skips_rendering() {
        // No vars + no `{{ }}` → straight parse.
        let yaml = r#"
name: plain
source:
  kind: memory
destination:
  kind: memory
"#;
        let spec = AirwayPipelineSpec::from_yaml_with_vars(yaml, None).expect("parse");
        assert_eq!(spec.name, "plain");
    }

    #[test]
    fn empty_object_skips_rendering() {
        // `{{ }}` present but vars is `{}` → treated as no-render. The
        // literal braces survive into the parsed spec verbatim (they're
        // valid YAML scalar text), proving rendering was skipped rather
        // than expanded.
        let vars = serde_json::json!({});
        let spec = AirwayPipelineSpec::from_yaml_with_vars(TEMPLATED, Some(&vars)).expect("parse");
        assert_eq!(spec.name, "shopify_{{ env }}");
        assert_eq!(spec.source.config["base_url"], "{{ base }}");
    }

    #[test]
    fn missing_variable_is_strict_error() {
        // `base` supplied, `env` missing → strict undefined errors out
        // rather than expanding to `shopify_`.
        let vars = serde_json::json!({ "base": "https://x" });
        let err = AirwayPipelineSpec::from_yaml_with_vars(TEMPLATED, Some(&vars)).unwrap_err();
        assert!(
            err.to_string().contains("variable substitution failed"),
            "got: {err}"
        );
    }

    #[test]
    fn non_object_variables_rejected() {
        let vars = serde_json::json!(["not", "a", "map"]);
        let err = AirwayPipelineSpec::from_yaml_with_vars(TEMPLATED, Some(&vars)).unwrap_err();
        assert!(err.to_string().contains("must be a mapping/object"));
    }
}
