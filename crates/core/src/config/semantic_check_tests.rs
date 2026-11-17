use std::path::{Path, PathBuf};

use oxy_compile::{CompiledConfig, SemanticSource, build_compiled_config};

use super::check_compiled_semantic_model;

fn demo_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../demo_project")
}

/// Every `*.{suffix}` under `root`, as `(workspace-relative path, text)`.
fn files(root: &Path, suffix: &str) -> Vec<(String, String)> {
    fn walk(dir: &Path, root: &Path, suffix: &str, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).expect("readable").flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, suffix, out);
            } else if path.to_string_lossy().ends_with(suffix) {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.push((rel, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, suffix, &mut out);
    out
}

fn sources(files: &[(String, String)]) -> Vec<SemanticSource<'_>> {
    files
        .iter()
        .map(|(file_path, text)| SemanticSource { file_path, text })
        .collect()
}

fn demo_config() -> CompiledConfig {
    let text = std::fs::read_to_string(demo_project().join("config.yml")).unwrap();
    let value: serde_json::Value = serde_yaml::from_str(&text).unwrap();
    build_compiled_config(value, Some("config.yml")).expect("demo config compiles")
}

/// The model every new demo workspace is seeded with builds, under its own
/// `config.yml`'s dialects. If this fails, the gate would refuse a model the
/// product ships.
#[test]
fn the_shipped_demo_model_builds() {
    let root = demo_project();
    let (views, topics) = (files(&root, ".view.yml"), files(&root, ".topic.yml"));
    assert!(!views.is_empty() && !topics.is_empty());
    let config = demo_config();
    check_compiled_semantic_model(Some(&config), &sources(&views), &sources(&topics))
        .expect("the demo model builds");
    // And without a config, the layer alone.
    check_compiled_semantic_model(None, &sources(&views), &sources(&topics))
        .expect("the demo layer parses");
}

#[test]
fn a_view_that_does_not_parse_is_reported_against_its_file() {
    let root = demo_project();
    let mut views = files(&root, ".view.yml");
    let topics = files(&root, ".topic.yml");
    views.push((
        "semantics/views/broken.view.yml".to_string(),
        "name: broken\ndimensions: 5\n".to_string(),
    ));
    let error =
        check_compiled_semantic_model(Some(&demo_config()), &sources(&views), &sources(&topics))
            .expect_err("a view whose dimensions are not a list does not build");
    assert_eq!(
        error.file_path.as_deref(),
        Some("semantics/views/broken.view.yml")
    );
}

#[test]
fn a_topic_naming_a_view_that_does_not_exist_does_not_build() {
    let root = demo_project();
    let views = files(&root, ".view.yml");
    let mut topics = files(&root, ".topic.yml");
    topics.push((
        "semantics/topics/ghost.topic.yml".to_string(),
        "name: ghost\ndescription: names a view nobody wrote\nbase_view: not_a_view\nviews:\n  - not_a_view\n"
            .to_string(),
    ));
    assert!(
        check_compiled_semantic_model(Some(&demo_config()), &sources(&views), &sources(&topics))
            .is_err(),
        "a dangling topic must not reach a pod"
    );
}

#[test]
fn an_empty_model_builds() {
    check_compiled_semantic_model(Some(&demo_config()), &[], &[]).expect("nothing to build");
}
