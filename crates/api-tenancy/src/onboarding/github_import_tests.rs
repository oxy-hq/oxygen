//! What the import checks in the repository before it connects it.

use std::path::Path;

use axum::http::StatusCode;

use super::{config_path_in_repo, not_an_oxy_project, require_compiles_from_git};

#[test]
fn config_is_looked_up_at_the_root_or_inside_the_subdirectory() {
    assert_eq!(config_path_in_repo(None), "config.yml");
    assert_eq!(
        config_path_in_repo(Some(Path::new("data/oxy"))),
        "data/oxy/config.yml"
    );
    assert_eq!(
        config_path_in_repo(Some(Path::new("analytics/"))),
        "analytics/config.yml"
    );
}

#[test]
fn a_repository_without_config_is_refused_with_the_words_the_clone_used() {
    let (status, msg) = not_an_oxy_project("https://github.com/acme/repo.git", None);
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        msg,
        "Repository 'https://github.com/acme/repo.git' does not appear to be an Oxygen project — no config.yml found."
    );
    let (_, msg) = not_an_oxy_project("u", Some(Path::new("data/oxy")));
    assert!(
        msg.ends_with("no config.yml found in subdirectory 'data/oxy'."),
        "{msg}"
    );
}

#[test]
fn an_import_is_refused_where_nothing_compiles_from_git() {
    assert!(require_compiles_from_git(true).is_ok());
    let (status, msg) = require_compiles_from_git(false).unwrap_err();
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(msg.contains("compile_reconcile"), "{msg}");
}
