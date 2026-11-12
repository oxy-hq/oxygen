//! What a cloud-mode `oxy serve` starts for the Slack integration once it is
//! configured: the Socket Mode connection, when an app-level token is set, and
//! the hourly sweep of stale Slack rows. Called by `oxy-server`'s `serve`.

/// Start the Slack integration's background work, reading its configuration
/// from the environment. Logs and returns when Slack is disabled or
/// misconfigured: webhooks then answer 503, and nothing else starts.
pub fn start_background() {
    use super::config::SlackConfig;
    match SlackConfig::from_env() {
        SlackConfig::Enabled(cfg) => {
            tracing::info!("Slack integration enabled");

            // Socket Mode: open a persistent WebSocket connection when an
            // app-level token is configured. HTTP webhooks remain active
            // regardless — Socket Mode is purely additive.
            if let Some(app_level_token) = cfg.app_level_token.clone() {
                tracing::info!("slack: Socket Mode enabled (OXY_SLACK_APP_LEVEL_TOKEN set)");
                tokio::spawn(async move {
                    super::socket_mode::run_socket_loop(app_level_token).await;
                });
            } else {
                tracing::info!("slack: using HTTP webhooks (OXY_SLACK_APP_LEVEL_TOKEN unset)");
            }

            // Background cleanup: delete stale Slack rows hourly.
            // - slack_oauth_states: TTL 15 min, but we keep rows 7 days for audit.
            // - slack_seen_events: keep rows 10 minutes (just enough for Slack retry window).
            tokio::spawn(async {
                use chrono::Duration as CDuration;
                use tokio::time::{Duration, sleep};
                loop {
                    sleep(Duration::from_secs(3600)).await;
                    match super::oauth::state::sweep_expired().await {
                        Ok(n) if n > 0 => {
                            tracing::info!("cleaned up {n} expired slack_oauth_states rows")
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("slack_oauth_states cleanup failed: {e}"),
                    }
                    match super::services::seen_events::SeenEventsService::sweep(
                        CDuration::minutes(10),
                    )
                    .await
                    {
                        Ok(n) if n > 0 => {
                            tracing::info!("cleaned up {n} old slack_seen_events rows")
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("slack_seen_events cleanup failed: {e}"),
                    }
                }
            });
        }
        SlackConfig::Disabled => {
            tracing::info!("Slack integration disabled via OXY_SLACK_ENABLED=false");
        }
        SlackConfig::Misconfigured => {
            tracing::warn!(
                "Slack integration is misconfigured — one or more of \
                 OXY_SLACK_CLIENT_ID / OXY_SLACK_CLIENT_SECRET / \
                 OXY_SLACK_SIGNING_SECRET / OXY_SLACK_APP_BASE_URL is unset. \
                 Webhooks will return 503."
            );
        }
    }
}
