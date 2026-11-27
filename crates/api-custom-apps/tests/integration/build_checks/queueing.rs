//! Who queues a build's checks, and what the queue is told about the task.

use super::*;

#[tokio::test]
async fn a_staging_publish_queues_one_attempt_with_its_rows_before_it_answers() {
    let t = seeded_tenant().await;
    let published = publish_staging(&t, "chk-queue", "cq-1").await;
    let (app, build) = models(&t, &published).await;
    assert_eq!(
        build.sdk_version.as_deref(),
        Some("2.30.0"),
        "the publish recorded it"
    );

    let tasks = queued(&t.db, app.id).await;
    assert_eq!(tasks.len(), 1, "one task per staging move: {tasks:?}");
    let run_id = format!("{BUILD_CHECKS_KIND}:staging:{}:{}:1", app.id, build.id);
    assert_eq!(tasks[0].0, run_id);
    let queued_rows = rows(&t.db, build.id).await;
    let kinds: Vec<(&str, i32, &str)> = queued_rows
        .iter()
        .map(|r| (r.kind.as_str(), r.attempt, r.status.as_str()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("design_system", 1, "queued"),
            ("functions", 1, "queued"),
            ("loads", 1, "queued"),
            ("validation", 1, "queued"),
        ]
    );
    assert!(
        queued_rows
            .iter()
            .all(|r| r.run_id.as_deref() == Some(run_id.as_str()) && r.runner.is_none())
    );

    // The same move again (a staging rollback to this build) queues nothing.
    let warnings = queue::after_staging_move_of(&t.db, &app, &build).await;
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(queued(&t.db, app.id).await.len(), 1);
    assert_eq!(rows(&t.db, build.id).await.len(), 4);
}

#[tokio::test]
async fn a_rerun_is_refused_for_a_build_staging_no_longer_serves_and_a_rollback_queues() {
    let t = seeded_tenant().await;
    let first = publish_staging(&t, "chk-move", "cm-1").await;
    let (app, build_a) = models(&t, &first).await;
    let second = publish_staging(&t, "chk-move", "cm-2").await;
    let (_, build_b) = models(&t, &second).await;
    let refused = queue::rerun(&t.db, &app, &build_a).await;
    assert!(
        matches!(&refused, Err(RerunRefusal::NotOnStaging { build }) if build == "cm-1"),
        "{refused:?}"
    );
    assert_eq!(refused.unwrap_err().code(), "build_not_on_staging");

    // Forget A's checks, then roll staging back to it: the promote path
    // queues them again.
    for statement in [
        "DELETE FROM agentic_task_queue WHERE task_id LIKE $1",
        "DELETE FROM agentic_runs WHERE id LIKE $1",
    ] {
        let like = format!("{BUILD_CHECKS_KIND}:staging:{}:{}:%", app.id, build_a.id);
        sql(&t.db, statement, vec![like.into()]).await;
    }
    app_build_checks::Entity::delete_many()
        .filter(app_build_checks::Column::BuildId.eq(build_a.id))
        .exec(&t.db)
        .await
        .unwrap();
    let request = serde_json::from_value(json!({ "to": build_a.build_id })).unwrap();
    oxy_api_custom_apps::custom_apps_promote::act::rollback(
        &t.db,
        &t.guest(),
        app.id,
        "staging",
        &request,
    )
    .await
    .expect("staging rolls back");
    let tasks: Vec<String> = queued(&t.db, app.id)
        .await
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    let for_a = format!("{BUILD_CHECKS_KIND}:staging:{}:{}:1", app.id, build_a.id);
    let for_b = format!("{BUILD_CHECKS_KIND}:staging:{}:{}:1", app.id, build_b.id);
    assert_eq!(tasks, [for_b, for_a]);
}

/// The checks are the platform's work on a staff surface: filed as a daemon
/// in every list that tells one from the tenant's own work (the run feed,
/// backend and frontend, and workspace health), and handed to the worker
/// fleet by `oxy-server`, which `oxy-app` refuses to boot without.
#[test]
fn the_checks_task_is_a_platform_daemon_the_fleet_is_handed() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let quoted = format!("\"{BUILD_CHECKS_KIND}\"");
    for (file, list) in [
        (
            "../agentic/runtime/src/lifecycle/crud/system_sources.rs",
            "pub const SYSTEM_SOURCE_TYPES: &[&str] = &[",
        ),
        (
            "../usage-report/src/customer_runs.rs",
            "const NON_WORKSPACE_RUN_SOURCES: &[&str] = &[",
        ),
        (
            "../../web-app/src/pages/ide/coordinator/components/constants.ts",
            "const SYSTEM_SOURCE_TYPES: readonly string[] = [",
        ),
        (
            "../app-core/src/seams.rs",
            "pub const CUSTOM_APP_TASK_KINDS: [&str; ",
        ),
    ] {
        let source = std::fs::read_to_string(root.join(file)).expect(file);
        let start = source
            .find(list)
            .unwrap_or_else(|| panic!("{file} no longer declares `{list}`"));
        let body = &source[start..];
        let body = &body[..body.find("];").expect("the list ends")];
        assert!(
            body.contains(&quoted),
            "{file}: `{BUILD_CHECKS_KIND}` is missing"
        );
    }
    let seams = std::fs::read_to_string(root.join("../server/src/custom_apps_seams.rs"))
        .expect("read custom_apps_seams.rs");
    let squeezed: String = seams.split_whitespace().collect();
    assert!(
        squeezed.contains(
            "TaskSeam{kind:oxy_api_custom_apps::custom_apps_build_checks::BUILD_CHECKS_KIND,"
        ),
        "`custom_apps_seams::tasks` no longer hands in the build checks executor"
    );
}
