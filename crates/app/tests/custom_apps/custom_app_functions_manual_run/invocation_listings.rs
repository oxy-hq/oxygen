//! The invocation listings: `GET /admin/apps/{id}/invocations` and the
//! per-function `GET …/functions/{name}/invocations`, as staff with reach
//! read them. Who may read which rows is `invocation_reach`'s.
//!
//! - they filter by environment, by build (the publish's build id and the
//!   build's UUID), by function and by `limit`, carry the environment and both
//!   build ids, and answer `[]` for a build they do not know;
//! - **nothing reads across apps**: another app's build matches nothing.

use axum::http::StatusCode;
use uuid::Uuid;

use super::get_admin;
use super::readback::{
    APP, OTHER_APP, PRODUCTION_BUILD, STAGING_BUILD, build_pk, ids, ran, two_builds,
};
use crate::custom_app_functions_fixture::seeded_tenant;
use crate::staging_functions::{make_guest_staff, production_host, staging_host};

#[tokio::test]
async fn invocations_filter_by_environment_build_function_and_limit() {
    let t = seeded_tenant().await;
    let app_id = two_builds(&t, APP, PRODUCTION_BUILD, STAGING_BUILD).await;
    let other_app = two_builds(&t, OTHER_APP, "rb-other-prod", "rb-other-stg").await;
    make_guest_staff();

    let live = ran(&t, APP, "whoami", &production_host(&t, APP))
        .await
        .to_string();
    let staged = ran(&t, APP, "whoami", &staging_host(&t, APP))
        .await
        .to_string();
    let wrote = ran(&t, APP, "writes", &staging_host(&t, APP))
        .await
        .to_string();
    let elsewhere = ran(&t, OTHER_APP, "whoami", &staging_host(&t, OTHER_APP)).await;
    let (production_pk, staging_pk) = (
        build_pk(&t, app_id, PRODUCTION_BUILD).await,
        build_pk(&t, app_id, STAGING_BUILD).await,
    );
    let list = |query: String| async move {
        let (status, body) = get_admin(&format!("/apps/{app_id}/invocations{query}")).await;
        assert_eq!(status, StatusCode::OK, "{query}: {body}");
        body["invocations"].clone()
    };

    // Everything the app ran, newest first — and nothing another app ran.
    let all = list(String::new()).await;
    assert_eq!(ids(&all), vec![wrote.clone(), staged.clone(), live.clone()]);
    let newest = &all[0];
    assert_eq!(newest["function_name"], "writes");
    assert_eq!(newest["environment"], "staging");
    assert_eq!(newest["build_id"], STAGING_BUILD);
    assert_eq!(newest["build_uuid"], staging_pk.to_string());
    assert_eq!(newest["mode"], "route");
    let oldest = &all[2];
    assert_eq!(oldest["environment"], "production");
    assert_eq!(oldest["build_id"], PRODUCTION_BUILD);
    assert_eq!(oldest["build_uuid"], production_pk.to_string());

    assert_eq!(
        ids(&list("?environment=staging".into()).await),
        vec![wrote.clone(), staged.clone()]
    );
    assert_eq!(
        ids(&list("?environment=production".into()).await),
        vec![live.clone()]
    );
    assert_eq!(
        ids(&list("?environment=dev-a1".into()).await),
        Vec::<String>::new()
    );

    // A build by the publish's build id, and by its UUID.
    for build in [STAGING_BUILD.to_string(), staging_pk.to_string()] {
        assert_eq!(
            ids(&list(format!("?build={build}")).await),
            vec![wrote.clone(), staged.clone()],
            "{build}"
        );
    }
    assert_eq!(
        ids(&list(format!("?build={PRODUCTION_BUILD}")).await),
        vec![live.clone()]
    );
    // An unknown build is an empty list — and so is another app's, by either name.
    let other_build = build_pk(&t, other_app, "rb-other-stg").await;
    for build in [
        "nope".to_string(),
        Uuid::new_v4().to_string(),
        "rb-other-stg".to_string(),
        other_build.to_string(),
    ] {
        assert_eq!(
            ids(&list(format!("?build={build}")).await),
            Vec::<String>::new(),
            "{build}"
        );
    }

    assert_eq!(
        ids(&list("?function=writes".into()).await),
        vec![wrote.clone()]
    );
    assert_eq!(
        ids(&list("?function=whoami&environment=staging".into()).await),
        vec![staged.clone()]
    );
    assert_eq!(ids(&list("?limit=1".into()).await), vec![wrote.clone()]);

    for (query, code) in [
        ("?limit=0", "invalid_limit"),
        ("?limit=201", "invalid_limit"),
        ("?limit=abc", "invalid_limit"),
        ("?environment=Prod", "invalid_environment"),
    ] {
        let (status, body) = get_admin(&format!("/apps/{app_id}/invocations{query}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}: {body}");
        assert_eq!(body["error"], code, "{query}: {body}");
    }

    // The other app's listing holds its own row and none of these.
    let (_, theirs) = get_admin(&format!("/apps/{other_app}/invocations")).await;
    assert_eq!(ids(&theirs["invocations"]), vec![elsewhere.to_string()]);
    let (status, body) = get_admin(&format!("/apps/{}/invocations", Uuid::new_v4())).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body["error"], "app_not_found");

    // The per-function listing is the same query with the path's function: a
    // bare array, the same new fields, the same filters and refusals.
    let per_function = format!("/apps/{app_id}/functions/whoami/invocations");
    let (status, mine) = get_admin(&per_function).await;
    assert_eq!(status, StatusCode::OK, "{mine}");
    assert_eq!(ids(&mine), vec![staged.clone(), live.clone()]);
    assert_eq!(mine[0]["build_id"], STAGING_BUILD);
    assert_eq!(mine[0]["environment"], "staging");
    let (_, narrowed) = get_admin(&format!(
        "{per_function}?environment=production&function=writes"
    ))
    .await;
    assert_eq!(
        ids(&narrowed),
        vec![live],
        "the path's function wins over ?function="
    );
    let (status, body) = get_admin(&format!("{per_function}?limit=0")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"], "invalid_limit");
}
