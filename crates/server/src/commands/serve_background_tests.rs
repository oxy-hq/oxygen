//! `serve`'s boot, run for real against `OXY_DATABASE_URL`.
//!
//! It builds the router and starts every boot job, the startup recovery pass and
//! the latency worker included, so it is the same unscoped writer of the shared
//! database as `served_router_tests`: it skips without a database, and
//! `.config/nextest.toml` pins this module into `serial-db`.

use oxy_app_core::serve_mode::ServeMode;

use crate::surface_seams;

/// The listener `serve`'s boot starts through the seam really attaches. A
/// NOTIFY never wakes the posting process directly (`announce` deliberately
/// skips the local bus), so a wake on this process's subscriber can only have
/// come back through the `LISTEN` connection the `chat_delivery_listener` job
/// opened. The announce repeats until the listener has attached (it connects in
/// the background), within a deadline.
///
/// Moved from `served_router_tests` in decompose BL-W: `api_router` starts no
/// job since then, and [`super::api_router_and_boot`] is what starts them.
#[tokio::test]
async fn the_served_router_starts_the_chat_delivery_listener() {
    if std::env::var("OXY_DATABASE_URL").is_err() {
        return;
    }
    let _router = super::api_router_and_boot(crate::router::ApiRouterArgs {
        mode: ServeMode::Cloud,
        enterprise: false,
        observability: None,
        startup_cwd: std::path::PathBuf::new(),
        shutdown_token: tokio_util::sync::CancellationToken::new(),
        disable_inprocess_workers: false,
        seams: surface_seams(),
    })
    .await
    .expect("router built");
    let channel = uuid::Uuid::new_v4();
    let mut rx = oxy_api_threads::chat::delivery::subscribe(channel);
    let db = oxy::database::client::establish_connection()
        .await
        .expect("database");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        oxy_api_threads::chat::delivery::announce(&db, channel).await;
        match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(())) => break,
            Ok(Err(e)) => panic!("the chat bus closed: {e}"),
            Err(_) => assert!(
                std::time::Instant::now() < deadline,
                "no wake within 10s: the chat delivery listener never attached, so \
                 a message posted on another replica would reach nobody here"
            ),
        }
    }
}
