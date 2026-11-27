//! The files an attempt keeps: `PUT …/checks/files/{name}` (the runner's) and
//! `GET …/checks/files/{name}` (the console's), on the filesystem build store
//! `test_db` points `OXY_STATE_DIR` at.

use std::path::PathBuf;

use oxy_custom_apps::build_store::check_files::{self, Attempt};

use super::*;

/// The smallest thing that starts like a PNG.
fn png(len: usize) -> Vec<u8> {
    let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    bytes.resize(len.max(8), 0);
    bytes
}

fn state_dir() -> PathBuf {
    std::env::var("OXY_STATE_DIR")
        .expect("test_db sets OXY_STATE_DIR")
        .into()
}

/// Where the contract says a build's check files live, on disk.
fn checks_dir(agent: &Agent, build: Uuid) -> PathBuf {
    state_dir().join(format!("customer-apps/{}/checks/{build}", agent.app.id))
}

/// `PUT …/files/{name}{query}` as the checker.
async fn put(
    agent: &Agent,
    build: Uuid,
    name_and_query: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let uri = format!("{}/files/{name_and_query}", checks(agent, build));
    let headers = [("x-api-key", TOKEN), ("content-type", content_type)];
    raw("PUT", &uri, &headers, bytes).await
}

/// `GET …/files/{name}{query}` with `credential`.
async fn get(
    agent: &Agent,
    build: Uuid,
    name_and_query: &str,
    credential: (&str, &str),
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let uri = format!("{}/files/{name_and_query}", checks(agent, build));
    raw("GET", &uri, &[credential], Vec::new()).await
}

const AT: &str = "?attempt=1&environment=staging";

#[tokio::test]
async fn a_file_round_trips_beside_the_build_and_the_console_reads_what_the_checker_cannot() {
    let agent = gated().await;
    let built = draft(&agent, "pf-1", None).await;
    let shot = png(4096);
    let text = br#"{"text":"Acme payroll: 4 people owed 12,900"}"#.to_vec();

    let kept = put(
        &agent,
        built,
        &format!("shot-staging-root-desk.png{AT}"),
        "image/png",
        shot.clone(),
    )
    .await;
    assert_eq!(kept.0, StatusCode::NO_CONTENT, "{:?}", refusal(&kept));
    assert!(kept.2.is_empty());
    // No attempt and no environment named: the newest staging attempt.
    let kept = put(
        &agent,
        built,
        "text-staging-root-desk.json",
        "application/json; charset=utf-8",
        text.clone(),
    )
    .await;
    assert_eq!(kept.0, StatusCode::NO_CONTENT, "{:?}", refusal(&kept));

    // On disk at the contract's key: beside the app's builds, never under one.
    let dir = checks_dir(&agent, built).join("staging/1");
    assert_eq!(
        std::fs::read(dir.join("shot-staging-root-desk.png")).unwrap(),
        shot
    );
    assert_eq!(
        std::fs::read(dir.join("text-staging-root-desk.json")).unwrap(),
        text
    );
    let builds = state_dir().join(format!("customer-apps/{}/builds", agent.app.id));
    assert!(builds.is_dir() && !dir.starts_with(&builds));

    // The console's door reads them back, as what they are.
    let person = ("cookie", agent.cookie.as_str());
    let read = get(
        &agent,
        built,
        &format!("shot-staging-root-desk.png{AT}"),
        person,
    )
    .await;
    assert_eq!(read.0, StatusCode::OK);
    assert_eq!(read.2, shot);
    let said = |name: &str| read.1.get(name).unwrap().to_str().unwrap().to_string();
    assert_eq!(said("content-type"), "image/png");
    assert_eq!(said("cache-control"), "private, max-age=3600");
    assert_eq!(said("x-content-type-options"), "nosniff");
    let read = get(&agent, built, "text-staging-root-desk.json", person).await;
    assert_eq!((read.0, read.2), (StatusCode::OK, text));
    assert_eq!(read.1.get("content-type").unwrap(), "application/json");
    let missing = get(&agent, built, &format!("diff-root-desk.png{AT}"), person).await;
    assert_eq!(missing.0, StatusCode::NOT_FOUND);
    let odd = get(&agent, built, &format!("Shot.png{AT}"), person).await;
    assert_eq!(
        refusal(&odd),
        (StatusCode::BAD_REQUEST, "invalid_name".to_string())
    );

    // The checker uploads; it cannot read one back. Nor can nobody.
    let checker = get(
        &agent,
        built,
        &format!("shot-staging-root-desk.png{AT}"),
        ("x-api-key", TOKEN),
    )
    .await;
    assert_eq!(checker.0, StatusCode::FORBIDDEN);
    let nobody = get(
        &agent,
        built,
        &format!("shot-staging-root-desk.png{AT}"),
        ("x-none", ""),
    )
    .await;
    assert_eq!(nobody.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn each_refusal_of_a_file_says_why() {
    let agent = gated().await;
    let built = draft(&agent, "pf-2", None).await;
    let refused = |answer: (StatusCode, HeaderMap, Vec<u8>)| refusal(&answer);
    let code = |status: StatusCode, code: &str| (status, code.to_string());

    for name in [
        "Shot.png",
        ".hidden.png",
        "a%2Fb.png",
        &format!("{}.png", "a".repeat(120)),
    ] {
        let answer = put(&agent, built, &format!("{name}{AT}"), "image/png", png(16)).await;
        assert_eq!(
            refused(answer),
            code(StatusCode::BAD_REQUEST, "invalid_name"),
            "{name}"
        );
    }
    for content_type in ["text/html", "image/svg+xml", "application/octet-stream"] {
        let answer = put(&agent, built, &format!("x.png{AT}"), content_type, png(16)).await;
        let unsupported = code(StatusCode::UNSUPPORTED_MEDIA_TYPE, "unsupported_type");
        assert_eq!(refused(answer), unsupported, "{content_type}");
    }
    // A body that is not what its type says is not kept as that type.
    let page = b"<html><script>alert(1)</script></html>".to_vec();
    let answer = put(
        &agent,
        built,
        &format!("x.png{AT}"),
        "image/png",
        page.clone(),
    )
    .await;
    assert_eq!(
        refused(answer),
        code(StatusCode::BAD_REQUEST, "invalid_file")
    );
    let answer = put(
        &agent,
        built,
        &format!("x.json{AT}"),
        "application/json",
        page,
    )
    .await;
    assert_eq!(
        refused(answer),
        code(StatusCode::BAD_REQUEST, "invalid_file")
    );

    let answer = put(
        &agent,
        built,
        &format!("big.png{AT}"),
        "image/png",
        png(2_000_001),
    )
    .await;
    assert_eq!(
        refused(answer),
        code(StatusCode::PAYLOAD_TOO_LARGE, "file_too_large")
    );
    let answer = put(
        &agent,
        built,
        &format!("big.png{AT}"),
        "image/png",
        png(2_000_000),
    )
    .await;
    assert_eq!(
        answer.0,
        StatusCode::NO_CONTENT,
        "exactly the limit is kept"
    );

    let answer = put(
        &agent,
        built,
        "x.png?attempt=1&environment=dev-a1",
        "image/png",
        png(16),
    )
    .await;
    assert_eq!(
        refused(answer),
        code(StatusCode::BAD_REQUEST, "invalid_environment")
    );
    let answer = put(
        &agent,
        built,
        "x.png?attempt=9&environment=staging",
        "image/png",
        png(16),
    )
    .await;
    assert_eq!(answer.0, StatusCode::NOT_FOUND, "no such attempt");
    let answer = put(
        &agent,
        built,
        "x.png?environment=production",
        "image/png",
        png(16),
    )
    .await;
    assert_eq!(
        answer.0,
        StatusCode::NOT_FOUND,
        "the build has no production attempt"
    );
    assert!(!checks_dir(&agent, built).join("production").exists());

    // Two hundred files, then no more; a name already kept is replaced.
    let at = Attempt {
        app_id: agent.app.id,
        build_pk: built,
        environment: "staging",
        attempt: 1,
    };
    let kept = check_files::names(&at).await.unwrap().len();
    assert_eq!(kept, 1, "only big.png so far");
    for n in kept..200 {
        check_files::put(&at, &format!("shot-{n}.png"), png(16))
            .await
            .unwrap();
    }
    let answer = put(
        &agent,
        built,
        &format!("one-more.png{AT}"),
        "image/png",
        png(16),
    )
    .await;
    assert_eq!(
        refused(answer),
        code(StatusCode::CONFLICT, "too_many_files")
    );
    let answer = put(
        &agent,
        built,
        &format!("shot-7.png{AT}"),
        "image/png",
        png(32),
    )
    .await;
    assert_eq!(
        answer.0,
        StatusCode::NO_CONTENT,
        "the same name again is one file"
    );
    assert_eq!(
        check_files::get(&at, "shot-7.png")
            .await
            .unwrap()
            .unwrap()
            .len(),
        32
    );
    assert_eq!(check_files::names(&at).await.unwrap().len(), 200);

    // Every check has its verdict: the attempt keeps no more files.
    pass_staging(&agent, built, json!([])).await;
    let answer = put(
        &agent,
        built,
        &format!("shot-7.png{AT}"),
        "image/png",
        png(16),
    )
    .await;
    assert_eq!(
        refused(answer),
        code(StatusCode::CONFLICT, "attempt_closed")
    );
    assert_eq!(
        check_files::get(&at, "shot-7.png")
            .await
            .unwrap()
            .unwrap()
            .len(),
        32
    );
}

/// A build's check files go when its row is pruned, and an app's when the
/// app is deleted; another build's stay.
#[tokio::test]
async fn files_are_pruned_with_their_build_and_deleted_with_the_app() {
    let agent = agent().await;
    with_checker(&agent.t).await;
    let old = draft(&agent, "pf-old", None).await;
    let kept = put(
        &agent,
        old,
        &format!("shot-staging-root-desk.png{AT}"),
        "image/png",
        png(64),
    )
    .await;
    assert_eq!(kept.0, StatusCode::NO_CONTENT, "{:?}", refusal(&kept));
    assert!(
        checks_dir(&agent, old)
            .join("staging/1/shot-staging-root-desk.png")
            .is_file()
    );

    // Twelve more drafts: retention keeps ten, and `pf-old` is not one of them.
    let mut newest = old;
    for n in 1..=12 {
        newest = draft(&agent, &format!("pf-{n:02}"), None).await;
    }
    let kept = put(
        &agent,
        newest,
        &format!("shot-staging-root-desk.png{AT}"),
        "image/png",
        png(64),
    )
    .await;
    assert_eq!(kept.0, StatusCode::NO_CONTENT, "{:?}", refusal(&kept));
    assert!(
        build_pk(&agent.t.db, agent.app.id, "pf-old")
            .await
            .is_none(),
        "pruned"
    );
    assert!(
        !checks_dir(&agent, old).exists(),
        "the pruned build's check files outlived its row"
    );
    assert!(
        checks_dir(&agent, newest)
            .join("staging/1/shot-staging-root-desk.png")
            .is_file()
    );

    // The app deleted: everything under its prefix goes, check files included.
    let uri = format!("/customer-apps/{}", agent.app.id);
    let (status, body) = person(&agent, "DELETE", &uri, None).await;
    assert!(status.is_success(), "{status}: {body}");
    let app_dir = state_dir().join(format!("customer-apps/{}", agent.app.id));
    assert!(!app_dir.exists(), "the deleted app's files outlived it");
}
