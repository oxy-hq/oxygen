//! The memory eval's merge step: after a pass that wrote notes
//! (`OXY_EVAL_MEMORY_WRITE`), merge the file once when the prompt can no
//! longer list every note, so the next pass reads all of what was learned.
//! Each merge is recorded beside the notes, in `<notes>.merges.jsonl`, with
//! its token usage, apart from the answers' cost.

use std::path::{Path, PathBuf};

use agentic_runtime::react::MergeReport;
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

use super::EvalRun;
use super::target_agentic::target_memory;
use super::types::AgenticInput;

/// Merges the pass's notes file, if it wrote one. Never fails the eval: a
/// failed merge leaves the notes as they were and says so in the record.
pub(super) async fn after_pass(run: &EvalRun, target: &AgenticInput) {
    let Some(notes) = merge_target(run.reads_working_copy(), |k| std::env::var_os(k)) else {
        return;
    };
    let outcome =
        agentic_pipeline::merge_eval_notes(run.project_ctx(), &target.config_path, &notes).await;
    if let Err(e) = &outcome {
        tracing::warn!(error = %e, notes = %notes.display(), "memory merge failed");
    }
    if let Err(e) = append(&notes, record(&outcome)).await {
        tracing::warn!(error = %e, "memory merge: could not write its record");
    }
}

/// The notes file a pass wrote, the one to merge: `oxy test` only.
fn merge_target(
    working_copy: bool,
    var: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    target_memory(working_copy, var).write
}

fn record(outcome: &Result<Option<MergeReport>, String>) -> Value {
    let at = chrono::Utc::now().to_rfc3339();
    match outcome {
        Ok(None) => json!({"at": at, "merged": false, "reason": "fits the prompt"}),
        Ok(Some(r)) => json!({
            "at": at,
            "merged": true,
            "before": r.before,
            "after": r.after,
            "model": r.usage.model,
            "input_tokens": r.usage.input_tokens,
            "output_tokens": r.usage.output_tokens,
            "cache_read_tokens": r.usage.cache_read_tokens,
            "cache_write_tokens": r.usage.cache_write_tokens,
            "cost_usd": r.usage.cost_usd,
        }),
        Err(e) => json!({"at": at, "merged": false, "error": e}),
    }
}

async fn append(notes: &Path, rec: Value) -> std::io::Result<()> {
    let mut path = notes.as_os_str().to_owned();
    path.push(".merges.jsonl");
    let mut f = (tokio::fs::OpenOptions::new().create(true).append(true))
        .open(PathBuf::from(path))
        .await?;
    f.write_all(format!("{rec}\n").as_bytes()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_oxy_test_pass_that_wrote_notes_merges() {
        let write = |k: &str| (k == "OXY_EVAL_MEMORY_WRITE").then(|| "/n.jsonl".into());
        let read = |k: &str| (k == "OXY_EVAL_MEMORY_READ").then(|| "/n.jsonl".into());
        assert_eq!(merge_target(true, write), Some(PathBuf::from("/n.jsonl")));
        assert_eq!(
            merge_target(false, write),
            None,
            "a server eval never merges"
        );
        assert_eq!(
            merge_target(true, read),
            None,
            "a read-only pass leaves its notes"
        );
    }

    #[tokio::test]
    async fn each_merge_is_recorded_beside_the_notes() {
        let dir = std::env::temp_dir().join(format!("oxy-merge-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let notes = dir.join("db.jsonl");
        let mut r = MergeReport {
            before: 63,
            after: 14,
            ..MergeReport::default()
        };
        r.usage.output_tokens = 900;
        append(&notes, record(&Ok(Some(r)))).await.unwrap();
        append(&notes, record(&Err("boom".into()))).await.unwrap();
        let text = std::fs::read_to_string(dir.join("db.jsonl.merges.jsonl")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let rows: Vec<Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(
            (rows[0]["before"].clone(), rows[0]["after"].clone()),
            (json!(63), json!(14))
        );
        assert_eq!(rows[0]["output_tokens"], 900);
        assert_eq!(
            (rows[1]["merged"].clone(), rows[1]["error"].clone()),
            (json!(false), json!("boom"))
        );
    }
}
