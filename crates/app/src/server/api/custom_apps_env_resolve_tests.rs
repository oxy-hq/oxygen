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
        "no dev slot exists before the dev-slot API"
    );
}

/// The functions fallback is the old `published.or(draft)`, and nothing more:
/// it never reaches past a production build, and staging never falls back.
#[test]
fn functions_fall_back_to_staging_only_for_an_app_never_promoted() {
    assert_eq!(
        builds(None, Some(2))
            .resolve_for_functions(&AppEnvironment::Production)
            .build_id,
        Some(Uuid::from_u128(2))
    );
    assert_eq!(
        builds(Some(1), Some(2))
            .resolve_for_functions(&AppEnvironment::Production)
            .build_id,
        Some(Uuid::from_u128(1))
    );
    assert_eq!(
        builds(Some(1), None)
            .resolve_for_functions(&AppEnvironment::Staging)
            .build_id,
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
