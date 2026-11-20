use std::sync::Arc;

use agentic_core::react::{ResultHandle, Tool, ToolCx};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::*;

struct NoResults;
impl ResultHandle for NoResults {
    fn put(&self, _: String, _: Vec<String>, _: Vec<Vec<Value>>) -> String {
        String::new()
    }
    fn rows(&self, _: &str) -> Option<Arc<Vec<Vec<Value>>>> {
        None
    }
    fn sql_for(&self, _: &str) -> Option<String> {
        None
    }
}

fn cx() -> ToolCx {
    ToolCx {
        run_id: "r".into(),
        call_id: "c".into(),
        cancel: CancellationToken::new(),
        results: Arc::new(NoResults),
    }
}

fn fixture() -> (tempfile::TempDir, Arc<WorkspaceFiles>) {
    let dir = tempfile::tempdir().unwrap();
    let fin = dir.path().join("financial");
    std::fs::create_dir_all(&fin).unwrap();
    std::fs::write(
        fin.join("account.view.yml"),
        "name: account\ndimensions:\n  - name: district_id\n",
    )
    .unwrap();
    std::fs::write(
        fin.join("financial.topic.yml"),
        "name: financial\nviews: [account]\n",
    )
    .unwrap();
    std::fs::write(fin.join("COVERAGE.md"), "gold notes district_id\n").unwrap();
    std::fs::write(
        dir.path().join("financial.test.yml"),
        "sql: SELECT district_id\n",
    )
    .unwrap();
    let globs = vec![
        "./financial/*.view.yml".to_string(),
        "./financial/*.topic.yml".to_string(),
    ];
    let files = WorkspaceFiles::load(dir.path(), &globs, None).unwrap();
    (dir, Arc::new(files))
}

fn entry(path: &str, kind: FileKind, content: &str) -> FileEntry {
    FileEntry {
        path: path.into(),
        kind,
        content: content.into(),
    }
}

async fn text(tool: &dyn Tool, args: Value) -> String {
    tool.call(args, &cx()).await.unwrap().text
}

#[test]
fn load_keeps_only_files_the_globs_resolve() {
    let (_d, files) = fixture();
    let paths: Vec<_> = files.paths();
    assert_eq!(
        paths,
        vec![
            ("financial/account.view.yml", FileKind::View),
            ("financial/financial.topic.yml", FileKind::Topic)
        ]
    );
}

#[test]
fn load_names_host_docs_and_skips_globs_that_escape_base_dir() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("base");
    std::fs::create_dir_all(&base).unwrap();
    std::fs::write(dir.path().join("secret.md"), "x").unwrap();
    std::fs::write(base.join("a.md"), "disk").unwrap();
    let globs = vec!["../secret.md".to_string(), "./a.md".to_string()];
    let docs = vec!["host doc".to_string()];
    let files = WorkspaceFiles::load(&base, &globs, Some(&docs)).unwrap();
    assert_eq!(files.paths(), vec![("docs/context_1.md", FileKind::Doc)]);
    let files = WorkspaceFiles::load(&base, &globs, None).unwrap();
    assert_eq!(files.paths(), vec![("a.md", FileKind::Doc)]);
}

#[tokio::test]
async fn read_refuses_a_file_outside_the_context_globs() {
    let (_d, files) = fixture();
    let err = ReadTool::new(files)
        .call(json!({"path": "financial.test.yml"}), &cx())
        .await
        .unwrap_err();
    assert_eq!(err.error, "no file financial.test.yml");
    assert_eq!(err.hint.as_deref(), Some("use search to find files"));
    assert!(!err.candidates.is_empty() && err.candidates.len() <= 5);
}

#[tokio::test]
async fn read_resolves_a_bare_view_name() {
    let (_d, files) = fixture();
    let out = text(&ReadTool::new(files), json!({"path": "account"})).await;
    assert!(out.starts_with("1\tname: account"), "{out}");
}

#[tokio::test]
async fn read_states_truncation() {
    let body: String = (1..=10).map(|i| format!("l{i}\n")).collect();
    let files = Arc::new(WorkspaceFiles::from_entries(vec![entry(
        "a.md",
        FileKind::Doc,
        &body,
    )]));
    let out = text(&ReadTool::new(files), json!({"path": "a.md", "limit": 3})).await;
    assert!(out.ends_with("… lines 1-3 of 10 shown"), "{out}");
}

#[tokio::test]
async fn read_with_a_huge_limit_does_not_overflow() {
    let body: String = (1..=10).map(|i| format!("l{i}\n")).collect();
    let files = Arc::new(WorkspaceFiles::from_entries(vec![entry(
        "a.md",
        FileKind::Doc,
        &body,
    )]));
    let out = text(
        &ReadTool::new(files),
        json!({"path": "a.md", "offset": 4, "limit": u64::MAX}),
    )
    .await;
    assert!(
        out.starts_with("4\tl4") && out.ends_with("… lines 4-10 of 10 shown"),
        "{out}"
    );
}

#[tokio::test]
async fn search_finds_lines_case_insensitively() {
    let (_d, files) = fixture();
    let out = text(&SearchTool::new(files), json!({"pattern": "DISTRICT"})).await;
    assert!(
        out.contains("financial/account.view.yml:3:") && out.contains("district_id"),
        "{out}"
    );
}

#[tokio::test]
async fn search_filters_by_kind() {
    let files = Arc::new(WorkspaceFiles::from_entries(vec![
        entry("v.view.yml", FileKind::View, "alpha\n"),
        entry("t.topic.yml", FileKind::Topic, "alpha\n"),
    ]));
    let out = text(
        &SearchTool::new(files),
        json!({"pattern": "alpha", "kind": "topic"}),
    )
    .await;
    assert!(
        out.contains("t.topic.yml:1") && !out.contains("v.view.yml"),
        "{out}"
    );
}

#[tokio::test]
async fn search_caps_matches_at_50() {
    let body = "hit\n".repeat(60);
    let files = Arc::new(WorkspaceFiles::from_entries(vec![entry(
        "a.md",
        FileKind::Doc,
        &body,
    )]));
    let out = text(&SearchTool::new(files), json!({"pattern": "hit"})).await;
    assert_eq!(out.lines().filter(|l| l.starts_with("a.md:")).count(), 50);
    assert!(
        out.ends_with("… 10 more matches (narrow the pattern)"),
        "{out}"
    );
}

#[tokio::test]
async fn search_without_a_match_lists_the_kinds_that_exist() {
    let (_d, files) = fixture();
    let out = text(&SearchTool::new(files), json!({"pattern": "zzz"})).await;
    assert_eq!(out, "no matches for \"zzz\"; kinds that exist: topic, view");
}

#[test]
fn load_with_empty_host_docs_drops_globbed_markdown() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.md"), "disk").unwrap();
    let globs = vec!["./*.md".to_string()];
    let files = WorkspaceFiles::load(dir.path(), &globs, Some(&[])).unwrap();
    assert!(files.paths().is_empty());
}

#[test]
fn broad_globs_admit_only_context_file_kinds() {
    let (dir, _) = fixture();
    let fin = dir.path().join("financial");
    for f in [
        "disputed.json",
        "x.test.yml",
        "a.automation.yml",
        "b.procedure.yaml",
    ] {
        std::fs::write(fin.join(f), "secret").unwrap();
    }
    std::fs::write(fin.join("q.sql"), "select 1").unwrap();
    for g in ["./financial/*", "./**/*"] {
        let files = WorkspaceFiles::load(dir.path(), &[g.to_string()], None).unwrap();
        let mut paths: Vec<_> = files.paths().into_iter().map(|(p, _)| p).collect();
        paths.sort();
        assert_eq!(
            paths,
            vec![
                "financial/COVERAGE.md",
                "financial/account.view.yml",
                "financial/financial.topic.yml",
                "financial/q.sql"
            ],
            "glob {g}"
        );
    }
}

#[cfg(unix)]
#[test]
fn load_skips_a_symlink_that_leaves_base_dir_and_oversized_files() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("secret.sql"), "select secret").unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.sql"),
        dir.path().join("link.sql"),
    )
    .unwrap();
    std::fs::write(dir.path().join("ok.sql"), "select 1").unwrap();
    std::fs::write(dir.path().join("big.sql"), vec![b'a'; 1024 * 1024 + 1]).unwrap();
    let files = WorkspaceFiles::load(dir.path(), &["*.sql".to_string()], None).unwrap();
    assert_eq!(files.paths(), vec![("ok.sql", FileKind::Sql)]);
}

#[tokio::test]
async fn read_past_the_end_is_a_failure() {
    let files = Arc::new(WorkspaceFiles::from_entries(vec![entry(
        "a.md",
        FileKind::Doc,
        "x\ny\n",
    )]));
    let err = ReadTool::new(files)
        .call(json!({"path": "a.md", "offset": 9}), &cx())
        .await
        .unwrap_err();
    assert_eq!(err.error, "offset 9 is past the end (2 lines)");
}
