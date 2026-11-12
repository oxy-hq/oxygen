use crate::client::SlackClient;
use crate::socket_mode::envelope::SocketEvent;
use crate::webhooks::events::dispatch as dispatch_event;
use crate::webhooks::interactivity::dispatch_interactivity;
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Infinite loop — opens a Socket Mode connection, reads events, reconnects.
/// Never returns under normal operation.
pub async fn run_socket_loop(app_level_token: String) {
    let client = SlackClient::new();
    let mut backoff = MIN_BACKOFF;

    loop {
        match run_once(&client, &app_level_token).await {
            Ok(()) => {
                // Clean close (disconnect frame) — reset backoff and loop.
                backoff = MIN_BACKOFF;
                tracing::info!("slack socket mode: disconnected cleanly, reopening");
            }
            Err(e) => {
                tracing::warn!(
                    "slack socket mode: connection failed: {e} — retrying in {:?}",
                    backoff
                );
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(MAX_BACKOFF);
            }
        }
    }
}

async fn run_once(
    client: &SlackClient,
    app_level_token: &str,
) -> Result<(), oxy_shared::errors::OxyError> {
    let url = client.apps_connections_open(app_level_token).await?;
    tracing::info!("slack socket mode: connecting to {}", redact_url(&url));

    let (mut ws, _resp) = connect_async(&url)
        .await
        .map_err(|e| oxy_shared::errors::OxyError::RuntimeError(format!("ws connect: {e}")))?;
    tracing::info!("slack socket mode: connected");

    while let Some(msg) = ws.next().await {
        let msg =
            msg.map_err(|e| oxy_shared::errors::OxyError::RuntimeError(format!("ws read: {e}")))?;
        let Message::Text(text) = msg else { continue };

        let event: SocketEvent = match serde_json::from_str(&text) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    "slack socket mode: failed to decode frame: {e} raw={}",
                    text
                );
                continue;
            }
        };

        match event {
            SocketEvent::Hello => {
                tracing::info!("slack socket mode: hello");
            }
            SocketEvent::EventsApi {
                envelope_id,
                payload,
            } => {
                // Ack first (3-second SLA), then dispatch asynchronously.
                let ack = serde_json::json!({"envelope_id": envelope_id}).to_string();
                if let Err(e) = ws.send(Message::text(ack)).await {
                    tracing::warn!("slack socket mode: ack failed: {e}");
                }
                // Socket mode runs outside the Axum stack, so there is no
                // AppState to take the node's rollup cache from; agent runs
                // started here compile their semantic queries to warehouse SQL.
                tokio::spawn(async move {
                    if let Err(e) = dispatch_event(payload, Default::default()).await {
                        tracing::error!("slack socket mode event dispatch: {e}");
                    }
                });
            }
            SocketEvent::Interactive {
                envelope_id,
                payload,
            } => {
                let ack = serde_json::json!({"envelope_id": envelope_id}).to_string();
                if let Err(e) = ws.send(Message::text(ack)).await {
                    tracing::warn!("slack socket mode: ack failed: {e}");
                }
                tokio::spawn(async move {
                    dispatch_interactivity(payload, Default::default()).await;
                });
            }
            SocketEvent::SlashCommands { envelope_id, .. } => {
                // Not used — ack and drop so Slack doesn't retry.
                let ack = serde_json::json!({"envelope_id": envelope_id}).to_string();
                let _ = ws.send(Message::text(ack)).await;
            }
            SocketEvent::Disconnect { reason } => {
                tracing::info!("slack socket mode: disconnect received: {:?}", reason);
                return Ok(());
            }
        }
    }

    // Stream ended without a Disconnect frame — treat as clean close.
    Ok(())
}

fn redact_url(url: &str) -> String {
    // The WSS URL query contains a single-use token; don't log it.
    if let Some((base, _)) = url.split_once('?') {
        format!("{base}?…")
    } else {
        url.to_string()
    }
}
