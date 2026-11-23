//! Memory for the eval harness's react runs: notes read from one file and
//! written to another, so a BIRD pass can learn (cold: write only) and a later
//! pass can use what it learned (warm: read only) without the warm pass
//! learning from itself. A served run has no memory yet.

use std::path::PathBuf;
use std::sync::Arc;

use agentic_runtime::react::{FileMemoryStore, MemoryStore, RunMemory, latest};

/// Where an eval run's notes come from and go to. Both `None` is no memory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvalMemory {
    /// Notes the prompt lists, read once per run.
    pub read: Option<PathBuf>,
    /// Where `remember` and `forget` append. `None`: a read-only run, with no
    /// memory tools and no reflection turn.
    pub write: Option<PathBuf>,
}

impl EvalMemory {
    pub fn is_off(&self) -> bool {
        self.read.is_none() && self.write.is_none()
    }

    /// The run's notes and its write store. A read file that is missing or
    /// bad ends the run: a warm pass must not run cold by accident.
    pub(crate) async fn load(&self) -> Result<RunMemory, String> {
        let notes = match &self.read {
            Some(path) => {
                if !tokio::fs::try_exists(path).await.unwrap_or(false) {
                    return Err(format!("memory notes {}: no such file", path.display()));
                }
                let facts = FileMemoryStore::new(path.clone()).facts().await;
                let facts = facts.map_err(|e| format!("memory notes {}: {e}", path.display()))?;
                latest(facts)
            }
            None => Vec::new(),
        };
        let writes =
            (self.write.clone()).map(|p| Arc::new(FileMemoryStore::new(p)) as Arc<dyn MemoryStore>);
        let quiet = false;
        Ok(RunMemory {
            notes,
            writes,
            quiet,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentic_runtime::react::{NoteFact, NoteOp};

    fn put(id: &str, text: &str) -> NoteFact {
        NoteFact {
            note_id: id.into(),
            op: NoteOp::Put,
            text: text.into(),
            run_id: "r".into(),
            recorded_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn read_only_memory_lists_notes_and_takes_no_writes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.jsonl");
        FileMemoryStore::new(&path)
            .append(put("n1", "use account.view"))
            .await
            .unwrap();
        let mem = EvalMemory {
            read: Some(path),
            write: None,
        };
        let run = mem.load().await.unwrap();
        assert_eq!(run.notes.len(), 1);
        assert_eq!(run.notes[0].text, "use account.view");
        assert!(run.writes.is_none());
    }

    #[tokio::test]
    async fn write_only_memory_lists_no_notes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.jsonl");
        FileMemoryStore::new(&path)
            .append(put("n1", "x"))
            .await
            .unwrap();
        let mem = EvalMemory {
            read: None,
            write: Some(path),
        };
        let run = mem.load().await.unwrap();
        assert!(run.notes.is_empty());
        assert!(run.writes.is_some());
    }

    #[tokio::test]
    async fn a_missing_or_bad_read_file_fails_the_run() {
        let dir = tempfile::tempdir().unwrap();
        let missing = EvalMemory {
            read: Some(dir.path().join("none.jsonl")),
            write: None,
        };
        assert!(missing.load().await.err().unwrap().contains("no such file"));
        let bad = dir.path().join("bad.jsonl");
        std::fs::write(&bad, "not json\n").unwrap();
        let err = EvalMemory {
            read: Some(bad),
            write: None,
        };
        assert!(err.load().await.err().unwrap().contains("bad.jsonl"));
    }

    /// End to end on the headless path: a cold run that made three queries
    /// saves a note, and a warm run lists it in its prompt with no writes.
    #[tokio::test]
    async fn a_cold_run_saves_a_note_that_a_warm_run_reads() {
        use crate::headless::EventDestination;
        use crate::react_run::tests::{call, react_run, turn};
        use crate::react_run::{loop_config, run_react_headless};
        use agentic_core::react::Decision;
        use serde_json::json;

        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join("notes.jsonl");
        let q = || call("query", json!({"sql": "select 5"}));
        let script = vec![
            q(),
            q(),
            q(),
            Decision::Reply(turn("It is 5.", vec![])),
            call("remember", json!({"text": "n lives in the one table"})),
        ];
        let cold = EvalMemory {
            read: None,
            write: Some(notes.clone()),
        };
        let run = react_run(script, dir.path()).with_memory(cold.load().await.unwrap());
        assert!(has_tool(&run, "remember") && has_tool(&run, "forget"));
        let answer = run_react_headless(run, "q".into(), EventDestination::Drain).await;
        assert_eq!(answer.unwrap(), "It is 5.");

        let warm = EvalMemory {
            read: Some(notes),
            write: None,
        };
        let run = react_run(vec![], dir.path()).with_memory(warm.load().await.unwrap());
        assert!(
            run.system.contains("n lives in the one table"),
            "{}",
            run.system
        );
        assert_eq!(run.system, run.analytics.toolset.system_prompt);
        assert!(loop_config(&run, "r".into()).reflect.is_none());
        assert!(
            !has_tool(&run, "remember"),
            "a read-only pass does not learn"
        );
    }

    fn has_tool(run: &crate::react_run::ReactRun, name: &str) -> bool {
        (run.analytics.toolset.tools.iter()).any(|t| t.def().name == name)
    }

    /// A run built without memory (the served and resume paths) has no notes
    /// and no reflection turn.
    #[test]
    fn a_run_without_memory_has_no_reflection() {
        use crate::react_run::loop_config;
        use crate::react_run::tests::react_run;
        let dir = tempfile::tempdir().unwrap();
        let run = react_run(vec![], dir.path());
        assert!(!run.system.contains("earlier runs"));
        assert!(loop_config(&run, "r".into()).reflect.is_none());
    }
}
