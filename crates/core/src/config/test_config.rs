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

impl TestFileConfig {
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
