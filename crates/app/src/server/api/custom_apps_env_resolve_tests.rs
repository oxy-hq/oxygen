use super::*;

fn builds(production: Option<u128>, staging: Option<u128>) -> EnvironmentBuilds {
    EnvironmentBuilds {
        production: production.map(Uuid::from_u128),
        staging: staging.map(Uuid::from_u128),
    }
}

#[test]
fn each_environment_resolves_to_its_own_build() {
    let b = builds(Some(1), Some(2));
    assert_eq!(
        b.resolve(&AppEnvironment::Production).build_id,
        Some(Uuid::from_u128(1))
    );
    assert_eq!(
        b.resolve(&AppEnvironment::Staging).build_id,
        Some(Uuid::from_u128(2))
    );
    assert_eq!(
        b.resolve(&AppEnvironment::Dev {
            handle: "luong".into()
        })
        .build_id,
        None,
        "a sandbox is not one of the cached fixed environments"
    );
}

/// The build the function readers resolve for `environment`, with no database
/// behind it: the connection is disconnected, so any statement panics. The
/// one build the fallback asks about (`custom_apps_agent_built`) is made
/// known by the test first, as a first request would have left it.
async fn for_functions(b: EnvironmentBuilds, environment: AppEnvironment) -> Option<Uuid> {
    let db = sea_orm::DatabaseConnection::default();
    b.resolve_for_functions(&db, &environment)
        .await
        .expect("no query")
        .build_id
}

/// The functions fallback is the old `published.or(draft)`, and nothing more:
/// it never reaches past a production build, and staging never falls back.
#[tokio::test]
async fn functions_fall_back_to_staging_only_for_an_app_never_promoted() {
    crate::server::api::custom_apps_agent_built::remember(Uuid::from_u128(2), false);
    assert_eq!(
        for_functions(builds(None, Some(2)), AppEnvironment::Production).await,
        Some(Uuid::from_u128(2))
    );
    assert_eq!(
        for_functions(builds(Some(1), Some(2)), AppEnvironment::Production).await,
        Some(Uuid::from_u128(1))
    );
    assert_eq!(
        for_functions(builds(Some(1), None), AppEnvironment::Staging).await,
        None,
        "staging never borrows production's build"
    );
    assert_eq!(
        builds(None, Some(2))
            .resolve(&AppEnvironment::Production)
            .build_id,
        None,
        "the strict resolve does not fall back"
    );
}

/// Whether naming `build` names staging for an app serving `b`: staging
/// serves it and production does not run it — production's answer being the
/// one the function runtime acts on (`production_runs`).
async fn names_staging(b: EnvironmentBuilds, build: Uuid) -> bool {
    let db = sea_orm::DatabaseConnection::default();
    let production = b.production_runs(&db).await.expect("no query");
    b.only_staging_serves(production, build)
}

/// A build names staging only when staging serves it and production does not
/// run it — by the same answer the function runtime resolves, the fallback
/// and its one exception included.
#[tokio::test]
async fn a_build_is_stagings_only_when_production_does_not_run_it() {
    use crate::server::api::custom_apps_agent_built::remember;
    let (one, two, three) = (Uuid::from_u128(1), Uuid::from_u128(2), Uuid::from_u128(3));
    let promoted = || builds(Some(1), Some(2));
    assert!(
        names_staging(promoted(), two).await,
        "staging's build, which production does not serve"
    );
    assert!(!names_staging(promoted(), one).await);
    assert!(
        !names_staging(promoted(), three).await,
        "a retained build no environment serves names none"
    );
    assert!(
        !names_staging(builds(Some(1), Some(1)), one).await,
        "a build both serve is production's"
    );
    assert!(!names_staging(builds(Some(1), None), one).await);

    // An app with no production build: a person's draft is what production
    // runs, so it is production's; a sandbox agent token's is not run there,
    // so it is staging's alone — the runtime and this reader agree.
    remember(two, false);
    assert!(
        !names_staging(builds(None, Some(2)), two).await,
        "a never-promoted app runs staging's build on the production path"
    );
    let by_token = Uuid::from_u128(0x7D);
    remember(by_token, true);
    let unpublished = || builds(None, Some(0x7D));
    assert_eq!(
        for_functions(unpublished(), AppEnvironment::Production).await,
        None,
        "production does not fall back to a token's draft"
    );
    assert!(
        names_staging(unpublished(), by_token).await,
        "so the draft is staging's build, not production's"
    );
}

fn app_row(published: Option<u128>, draft: Option<u128>) -> apps::Model {
    let now = chrono::Utc::now().fixed_offset();
    apps::Model {
        visibility: "org".to_string(),
        id: Uuid::from_u128(9),
        slug: "x".to_string(),
        name: "X".to_string(),
        org_id: Uuid::nil(),
        project_id: Uuid::nil(),
        branch: "main".to_string(),
        source_repo: String::new(),
        status: "created".to_string(),
        source_type: "s3".to_string(),
        source_config: serde_json::json!({}),
        bootstrap_pr_url: None,
        last_synced_at: None,
        manifest_override: None,
        published_at: None,
        repo_path: None,
        draft_build_id: draft.map(Uuid::from_u128),
        published_build_id: published.map(Uuid::from_u128),
        last_promoted_by: None,
        last_promoted_at: None,
        created_at: now,
        updated_at: now,
    }
}

/// Production is answered from the row in hand, with no query: the
/// connection here is disconnected, so any statement panics. Staging is
/// the control — it does read `app_environments`, and the fake catches it.
///
/// The never-promoted fallback is the one place production asks anything:
/// whether a sandbox agent token published the build it would fall back to
/// (`custom_apps_agent_built`). It asks once and then answers from memory, so
/// the build is made known here, as a first request would have left it.
#[tokio::test]
async fn production_resolves_from_the_app_row_without_a_query() {
    use futures::FutureExt;
    let db = sea_orm::DatabaseConnection::default();
    let production = AppEnvironment::Production;
    let promoted = app_row(Some(1), Some(2));
    let resolved = resolve_function_environment(&db, &promoted, &production)
        .await
        .expect("no query");
    assert_eq!(resolved.build_id, Some(Uuid::from_u128(1)));
    let never_promoted = app_row(None, Some(2));
    crate::server::api::custom_apps_agent_built::remember(Uuid::from_u128(2), false);
    let resolved = resolve_function_environment(&db, &never_promoted, &production)
        .await
        .expect("no query");
    assert_eq!(
        resolved.build_id,
        Some(Uuid::from_u128(2)),
        "main's fallback"
    );
    let resolved = resolve_environment(&db, &never_promoted, &production)
        .await
        .expect("no query");
    assert_eq!(
        resolved.build_id, None,
        "the strict resolve does not fall back"
    );

    let staging = std::panic::AssertUnwindSafe(resolve_function_environment(
        &db,
        &promoted,
        &AppEnvironment::Staging,
    ))
    .catch_unwind()
    .await;
    assert!(
        staging.is_err(),
        "control: staging queries, and the fake refuses it"
    );
}

/// The one exception to the fallback: a build a sandbox agent token published
/// is never what production falls back to. Production then serves nothing,
/// as an app with neither build does — and an app with a production build of
/// its own is never asked, whoever published its draft.
#[tokio::test]
async fn production_never_falls_back_to_a_build_a_token_published() {
    use crate::server::api::custom_apps_agent_built::remember;
    use futures::FutureExt;
    let db = sea_orm::DatabaseConnection::default();
    let production = AppEnvironment::Production;
    let (by_token, by_person) = (Uuid::from_u128(0x7A), Uuid::from_u128(0x7B));
    remember(by_token, true);
    remember(by_person, false);

    let unpublished = |draft: u128| app_row(None, Some(draft));
    let resolved = resolve_function_environment(&db, &unpublished(0x7A), &production)
        .await
        .expect("no query");
    assert_eq!(resolved.build_id, None, "no fallback to the token's draft");
    let resolved = resolve_function_environment(&db, &unpublished(0x7B), &production)
        .await
        .expect("no query");
    assert_eq!(resolved.build_id, Some(by_person), "a person's, as before");

    // Live: production serves its own build and asks nothing of the draft.
    let live = app_row(Some(1), Some(0x7A));
    let resolved = resolve_function_environment(&db, &live, &production)
        .await
        .expect("no query");
    assert_eq!(resolved.build_id, Some(Uuid::from_u128(1)));
    // Neither build: nothing to fall back to, and nothing to ask.
    let resolved = resolve_function_environment(&db, &app_row(None, None), &production)
        .await
        .expect("no query");
    assert_eq!(resolved.build_id, None);

    // Control: a draft nobody has asked about yet is asked about, once.
    let unknown = std::panic::AssertUnwindSafe(resolve_function_environment(
        &db,
        &unpublished(0x7C),
        &production,
    ))
    .catch_unwind()
    .await;
    assert!(unknown.is_err(), "the fallback reads who published it");
}

/// `sandbox_row` asks nothing for an environment that is not a sandbox (the
/// connection here is disconnected, so any statement panics), and does query
/// for one that is — the control.
#[tokio::test]
async fn sandbox_row_reads_only_for_a_sandbox() {
    use futures::FutureExt;
    let db = sea_orm::DatabaseConnection::default();
    let app = app_row(Some(1), Some(2));
    for fixed in [AppEnvironment::Production, AppEnvironment::Staging] {
        assert_eq!(
            sandbox_row(&db, app.id, &fixed).await.expect("no query"),
            None
        );
    }
    let sandbox = AppEnvironment::Dev {
        handle: "a1".into(),
    };
    let read = std::panic::AssertUnwindSafe(sandbox_row(&db, app.id, &sandbox))
        .catch_unwind()
        .await;
    assert!(
        read.is_err(),
        "control: a sandbox queries, and the fake refuses it"
    );
}

#[test]
fn a_missing_row_takes_the_column_and_a_row_wins_over_it() {
    let app = Uuid::from_u128(9);
    let column = Some(Uuid::from_u128(5));
    assert_eq!(from_row_or_column(app, "production", None, column), column);
    assert_eq!(
        from_row_or_column(app, "production", Some(Some(Uuid::from_u128(6))), column),
        Some(Uuid::from_u128(6))
    );
    assert_eq!(
        from_row_or_column(app, "staging", Some(None), column),
        None,
        "a row that serves nothing is an answer, not a gap"
    );
}
