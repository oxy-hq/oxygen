use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn default_test_concurrency() -> usize {
    5
}

fn default_test_runs() -> usize {
    3
}

/// How a case's answer is graded.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, Default, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Grader {
    /// LLM-as-judge compares the answer text with `expected`.
    #[default]
    Judge,
    /// Run the agent's final SQL and `expected_sql` against `database`, compare result sets.
    Execution,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestFileConfig {
    pub name: Option<String>,
    pub target: Option<String>,
    #[serde(default)]
    pub settings: TestSettings,
    pub cases: Vec<TestCase>,
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestSettings {
    #[serde(default = "default_test_concurrency")]
    pub concurrency: usize,
    #[serde(default = "default_test_runs")]
    pub runs: usize,
    pub judge_model: Option<String>,
    #[serde(default)]
    pub grader: Grader,
    /// Database the execution grader runs both queries against.
    #[serde(default)]
    pub database: Option<String>,
}

impl Default for TestSettings {
    fn default() -> Self {
        Self {
            concurrency: default_test_concurrency(),
            runs: default_test_runs(),
            judge_model: None,
            grader: Grader::default(),
            database: None,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestCase {
    #[serde(default)]
    pub name: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub expected: String,
    /// Gold SQL for the execution grader.
    #[serde(default)]
    pub expected_sql: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional: assert that this tool was invoked (deterministic check).
    /// TODO: implement tool-use verification in the eval pipeline.
    pub tool: Option<String>,
}

/// The agent a test file runs when it names none, beside it:
/// `tests/sales.agentic.test.yml` -> `tests/sales.agentic.yml`. The rule
/// `ConfigStorage::load_test_config` applies to a file on disk.
pub fn inferred_target(file_path: &str) -> Option<String> {
    file_path
        .strip_suffix(".test.yml")
        .map(|base| format!("{base}.yml"))
}

impl TestFileConfig {
    /// A compiled test file's body (`eval_test_definitions.definition`), read
    /// as `load_test_config` reads the file: its `target`, or the agent beside
    /// it.
    pub fn from_definition(file_path: &str, definition: serde_json::Value) -> Result<Self, String> {
        let mut config: Self = serde_json::from_value(definition)
            .map_err(|e| format!("Failed to deserialize test config {file_path}: {e}"))?;
        if config.target.is_none() {
            config.target = inferred_target(file_path);
        }
        Ok(config)
    }

    /// Check that every case carries what the selected grader needs.
    pub fn validate_grading(&self) -> Result<(), String> {
        match self.settings.grader {
            Grader::Judge => {
                for (i, c) in self.cases.iter().enumerate() {
                    if c.expected.trim().is_empty() {
                        return Err(format!(
                            "case {i}: `expected` is required when grader is `judge`"
                        ));
                    }
                }
            }
            Grader::Execution => {
                if self.settings.database.is_none() {
                    return Err(
                        "settings.database is required when grader is `execution`".to_string()
                    );
                }
                for (i, c) in self.cases.iter().enumerate() {
                    if c.expected_sql
                        .as_deref()
                        .is_none_or(|s| s.trim().is_empty())
                    {
                        return Err(format!(
                            "case {i}: `expected_sql` is required when grader is `execution`"
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(yaml: &str) -> TestFileConfig {
        serde_yaml::from_str(yaml).expect("test yaml parses")
    }

    #[test]
    fn a_compiled_test_file_reads_as_the_file_on_disk_does() {
        let def: serde_json::Value =
            serde_yaml::from_str("cases:\n  - prompt: p\n    expected: e\n").unwrap();
        let config = TestFileConfig::from_definition("tests/sales.agentic.test.yml", def).unwrap();
        assert_eq!(config.target.as_deref(), Some("tests/sales.agentic.yml"));
        assert_eq!(config.settings.runs, 3);

        let named: serde_json::Value =
            serde_yaml::from_str("target: agents/a.agentic.yml\ncases: []\n").unwrap();
        let config = TestFileConfig::from_definition("x.test.yml", named).unwrap();
        assert_eq!(config.target.as_deref(), Some("agents/a.agentic.yml"));

        let bad: serde_json::Value = serde_yaml::from_str("nope: 1\ncases: []\n").unwrap();
        assert!(TestFileConfig::from_definition("x.test.yml", bad).is_err());
    }

    #[test]
    fn execution_grader_requires_database_and_expected_sql() {
        let no_db = parse(
            "settings:\n  grader: execution\ncases:\n  - prompt: p\n    expected_sql: select 1\n",
        );
        let err = no_db.validate_grading().unwrap_err();
        assert!(err.contains("database"), "{err}");

        let no_sql = parse(
            "settings:\n  grader: execution\n  database: db\ncases:\n  - prompt: p\n    expected_sql: select 1\n  - prompt: q\n",
        );
        let err = no_sql.validate_grading().unwrap_err();
        assert!(err.contains("case 1"), "{err}");

        let ok = parse(
            "settings:\n  grader: execution\n  database: db\ncases:\n  - prompt: p\n    expected_sql: select 1\n",
        );
        assert!(ok.validate_grading().is_ok());
    }

    #[test]
    fn judge_grader_requires_expected() {
        let cfg = parse("cases:\n  - prompt: p\n");
        let err = cfg.validate_grading().unwrap_err();
        assert!(err.contains("case 0"), "{err}");
    }

    #[test]
    fn existing_test_files_still_parse() {
        let cfg = parse(include_str!(
            "../../../../examples/testing/analytics.agentic.test.yml"
        ));
        assert_eq!(cfg.settings.grader, Grader::Judge);
        assert!(cfg.validate_grading().is_ok());
    }
}
