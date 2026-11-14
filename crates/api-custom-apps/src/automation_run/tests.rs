use super::*;

/// The same rule, exercised through the real lookup rather than the helper.
///
/// A unit test on `matches_automation_id` alone does not bind the wiring:
/// restore `find(|r| r.name == automation_id)` and it still passes. This
/// goes through `compiled_automation`, so the call site is what is pinned.
///
/// `Origin::Disk`, so no database is needed — `list_automations` reads the
/// working copy and produces exactly the `AutomationEntry` shape the
/// compiled arm gets: `name` from the YAML, `file_path` from the path.
#[tokio::test]
async fn the_lookup_finds_an_automation_whose_yaml_renames_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(dir.path().join("config.yml"), "models: []\ndatabases: []\n")
        .await
        .expect("config");
    tokio::fs::create_dir_all(dir.path().join("procedures"))
        .await
        .expect("mkdir");
    // The shape 12 of the 18 shipped examples have.
    tokio::fs::write(
        dir.path().join("procedures/anonymize.automation.yml"),
        "name: anonymize_sample\ntasks: []\n",
    )
    .await
    .expect("automation");

    let manager = oxy::config::ConfigBuilder::new()
        .with_workspace_path(dir.path())
        .expect("workspace path")
        .build_with_working_copy(oxy::config::Origin::Disk, oxy::config::OnMissing::Empty)
        .await
        .expect("manager");

    assert!(
        matches!(
            compiled_automation(&manager, "anonymize").await,
            Lookup::Found(_)
        ),
        "the id is the filename; matching the YAML `name:` misses every \
         automation that renames itself, and on a diskless node that miss \
         is a permanent 503"
    );
}

/// The two sources must answer the same question the same way.
///
/// `automation_id` is a file's base name. The compiled arm used to match
/// `AutomationEntry::name`, which is the YAML `name:` when the file
/// declares one — twelve of the eighteen `examples/procedures/*.automation
/// .yml` declare one that differs from their filename. Every one of those
/// missed on a node with no working copy, and the miss becomes a `503
/// "a compile has been enqueued — retry shortly"` that recompiling cannot
/// clear, for an automation that IS in the revision.
#[test]
fn an_automation_is_found_by_its_filename_not_its_declared_name() {
    // The real shape: `anonymize.automation.yml` declaring
    // `name: anonymize_sample`.
    assert!(
        matches_automation_id("procedures/anonymize.automation.yml", "anonymize"),
        "the id is the filename, whatever the YAML calls itself"
    );
    assert!(
        !matches_automation_id("procedures/anonymize.automation.yml", "anonymize_sample"),
        "and the declared name is NOT an id — matching it would make the \
         compiled arm answer a question the FS arm never answers"
    );

    // The legacy extension, and a nested path.
    assert!(matches_automation_id(
        "workflows/staging/refresh.procedure.yml",
        "refresh"
    ));

    // A prefix is not a match, and neither is a different extension.
    assert!(!matches_automation_id(
        "procedures/anonymize_v2.automation.yml",
        "anonymize"
    ));
    assert!(!matches_automation_id(
        "procedures/anonymize.yml",
        "anonymize"
    ));
}
