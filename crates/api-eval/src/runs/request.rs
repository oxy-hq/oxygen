//! What an eval run is asked for, checked before anything is queued: the
//! request body, its rules (the ones `oxy test` applied to its flags), and the
//! payload the queued task carries. No I/O here.

use oxy::config::test_config::TestFileConfig;
use serde::{Deserialize, Serialize};

pub(crate) const TEST_FILE_SUFFIX: &str = ".test.yml";

/// `POST /api/{workspace_id}/evals/runs`.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateRunRequest {
    /// A workspace-relative `.test.yml`. Omitted runs every test file in the
    /// served revision.
    #[serde(default)]
    pub test_file: Option<String>,
    /// Only this case: a 0-based index, a case `name`, or its exact prompt.
    /// Needs `test_file`.
    #[serde(default)]
    pub case: Option<String>,
    /// Only the cases carrying this tag.
    #[serde(default)]
    pub tag: Option<String>,
    /// A results document (`oxy test --output-json`) whose recorded outputs
    /// are graded again instead of running the agent. Needs `test_file`; not
    /// with `case` or `tag`.
    #[serde(default)]
    pub regrade: Option<String>,
}

/// The rules `oxy test` applied to its flags, worded for the route.
pub fn validate(req: &CreateRunRequest) -> Result<(), String> {
    if let Some(file) = req.test_file.as_deref()
        && !file.ends_with(TEST_FILE_SUFFIX)
    {
        return Err(format!(
            "Unsupported test file: {file}. Evals run {TEST_FILE_SUFFIX} files only."
        ));
    }
    if req.case.is_some() && req.test_file.is_none() {
        return Err("`case` requires a specific `test_file`".to_string());
    }
    if req.regrade.is_some() {
        if req.test_file.is_none() {
            return Err("`regrade` requires the test file the results came from".to_string());
        }
        if req.case.is_some() || req.tag.is_some() {
            return Err("`regrade` cannot be combined with `case` or `tag`".to_string());
        }
    }
    Ok(())
}

/// `case` to a 0-based index: an integer index, a case `name`, or an exact
/// `prompt`, first match winning — `oxy test --case`'s rule.
pub fn resolve_case_index(
    config: &TestFileConfig,
    case: &str,
    file: &str,
) -> Result<usize, String> {
    if let Ok(idx) = case.parse::<usize>() {
        if idx >= config.cases.len() {
            return Err(format!(
                "Case index {idx} is out of bounds: {file} has {} case(s) (0-based)",
                config.cases.len()
            ));
        }
        return Ok(idx);
    }
    config
        .cases
        .iter()
        .position(|c| c.name.as_deref() == Some(case) || c.prompt == case)
        .ok_or_else(|| format!("No test case with name or prompt {case:?} found in {file}"))
}

/// The queued `eval_run` task's payload. The run row's id is the task's.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvalRunPayload {
    pub workspace_id: uuid::Uuid,
    /// The caller, so an `airhouse_managed` query runs as them.
    pub user_id: uuid::Uuid,
    #[serde(default)]
    pub role: Option<entity::workspace_members::WorkspaceRole>,
    /// Workspace-relative, in run order.
    pub test_files: Vec<String>,
    #[serde(default)]
    pub case_index: Option<usize>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub regrade: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(test_file: Option<&str>) -> CreateRunRequest {
        CreateRunRequest {
            test_file: test_file.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn the_flag_rules_of_oxy_test_hold() {
        assert!(validate(&req(None)).is_ok());
        assert!(validate(&req(Some("tests/a.test.yml"))).is_ok());
        let err = validate(&req(Some("agents/a.agentic.yml"))).unwrap_err();
        assert!(err.contains("Unsupported test file"), "{err}");

        let case_alone = CreateRunRequest {
            case: Some("0".into()),
            ..Default::default()
        };
        assert!(validate(&case_alone).unwrap_err().contains("case"));

        let regrade_alone = CreateRunRequest {
            regrade: Some("[]".into()),
            ..Default::default()
        };
        assert!(validate(&regrade_alone).unwrap_err().contains("regrade"));

        let regrade_with_tag = CreateRunRequest {
            test_file: Some("a.test.yml".into()),
            regrade: Some("[]".into()),
            tag: Some("t".into()),
            ..Default::default()
        };
        assert!(validate(&regrade_with_tag).unwrap_err().contains("cannot"));
    }

    #[test]
    fn an_unknown_field_is_refused() {
        let parsed: Result<CreateRunRequest, _> =
            serde_json::from_str(r#"{"test_file":"a.test.yml","min_accuracy":0.8}"#);
        assert!(parsed.is_err());
    }

    #[test]
    fn a_case_resolves_by_index_name_or_prompt() {
        let config: TestFileConfig = serde_yaml::from_str(
            "cases:\n  - name: first\n    prompt: p0\n    expected: e\n  - prompt: p1\n    expected: e\n",
        )
        .unwrap();
        assert_eq!(resolve_case_index(&config, "1", "f").unwrap(), 1);
        assert_eq!(resolve_case_index(&config, "first", "f").unwrap(), 0);
        assert_eq!(resolve_case_index(&config, "p1", "f").unwrap(), 1);
        assert!(
            resolve_case_index(&config, "2", "f")
                .unwrap_err()
                .contains("out of bounds")
        );
        assert!(
            resolve_case_index(&config, "nope", "f")
                .unwrap_err()
                .contains("No test case")
        );
    }

    #[test]
    fn the_payload_round_trips() {
        let payload = EvalRunPayload {
            workspace_id: uuid::Uuid::nil(),
            user_id: uuid::Uuid::nil(),
            role: Some(entity::workspace_members::WorkspaceRole::Member),
            test_files: vec!["tests/a.test.yml".into()],
            case_index: Some(0),
            tag: None,
            regrade: None,
        };
        let value = serde_json::to_value(&payload).unwrap();
        assert_eq!(
            serde_json::from_value::<EvalRunPayload>(value).unwrap(),
            payload
        );
    }
}
