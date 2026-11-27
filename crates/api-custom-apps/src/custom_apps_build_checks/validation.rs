//! The `validation` check: the bundle's recorded validation, the same fact the
//! promote blocker `validation_not_passed` reads (`app_builds.validation_status`).

use entity::app_builds;
use serde_json::json;

use super::kind::{CheckStatus, Finding, Outcome, Severity};

pub fn check(build: &app_builds::Model) -> Outcome {
    let extra = json!({
        "validation_status": build.validation_status,
        "validation_detail": build.validation_detail,
    });
    match build.validation_status.as_str() {
        "passed" => Outcome::with(CheckStatus::Passed, "bundle valid", &[], extra),
        "failed" => {
            let why = build
                .validation_detail
                .clone()
                .unwrap_or_else(|| "the bundle failed validation".to_string());
            let finding = Finding::new(Severity::Error, why.clone()).rule("validation_not_passed");
            Outcome::with(CheckStatus::Failed, why, &[finding], extra)
        }
        other => {
            let why = format!("validation has not reached a verdict (status `{other}`)");
            let finding = Finding::new(Severity::Error, why.clone());
            Outcome::with(CheckStatus::Error, why, &[finding], extra)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(status: &str, detail: Option<&str>) -> app_builds::Model {
        serde_json::from_value(json!({
            "id": uuid::Uuid::from_u128(1),
            "app_id": uuid::Uuid::from_u128(2),
            "build_id": "b1",
            "s3_prefix": "p/",
            "manifest_json": null,
            "created_at": "2026-11-20T00:00:00Z",
            "published_by": null,
            "published_via": null,
            "source_repo": null,
            "commit_sha": null,
            "source_branch": null,
            "validation_status": status,
            "validation_detail": detail,
            "semantic_revision_id": null,
            "published_token_id": null,
            "published_to": "staging",
            "sdk_version": null,
        }))
        .expect("a build row")
    }

    #[test]
    fn the_recorded_validation_is_the_verdict() {
        assert_eq!(check(&build("passed", None)).status, CheckStatus::Passed);
        let failed = check(&build("failed", Some("index.html is missing")));
        assert_eq!(failed.status, CheckStatus::Failed);
        assert_eq!(failed.summary, "index.html is missing");
        assert_eq!(
            failed.detail["findings"][0]["rule"],
            "validation_not_passed"
        );
        assert_eq!(check(&build("pending", None)).status, CheckStatus::Error);
    }
}
