use crate::config::SlackConfig;
use crate::signature::verify_request;
use crate::types::InteractivityPayload;
use crate::webhooks::handlers;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use chrono::Utc;
use oxy_app_core::preagg_cache_ctx::PreaggCacheCtx;

pub async fn handle_interactivity(
    headers: HeaderMap,
    State(app_state): State<oxy_app_core::AppState>,
    body: Bytes,
) -> impl IntoResponse {
    let cfg = match SlackConfig::cached().as_runtime() {
        Some(c) => c,
        None => return (StatusCode::SERVICE_UNAVAILABLE, "slack disabled").into_response(),
    };

    let ts = headers
        .get("x-slack-request-timestamp")
        .and_then(|v| v.to_str().ok());
    let sig = headers
        .get("x-slack-signature")
        .and_then(|v| v.to_str().ok());
    if let Err(e) = verify_request(&cfg.signing_secret, ts, sig, &body, Utc::now().timestamp()) {
        tracing::warn!("interactivity signature verify: {e}");
        return (StatusCode::UNAUTHORIZED, "invalid_signature").into_response();
    }

    let form: std::collections::HashMap<String, String> = match serde_urlencoded::from_bytes(&body)
    {
        Ok(f) => f,
        Err(_) => return (StatusCode::BAD_REQUEST, "bad form").into_response(),
    };
    let Some(raw) = form.get("payload") else {
        return (StatusCode::BAD_REQUEST, "missing payload").into_response();
    };
    let payload: InteractivityPayload = match serde_json::from_str(raw) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("interactivity decode: {e}");
            return (StatusCode::BAD_REQUEST, "bad payload").into_response();
        }
    };

    // The picker actions below start an agent run, so they need the node's
    // rollup cache the same way an inbound event does.
    let preagg = PreaggCacheCtx {
        cache: app_state.preagg_cache.clone(),
        renewal_threshold_secs: app_state.preagg_renewal_threshold_secs,
    };
    dispatch_interactivity(payload, preagg).await;
    (StatusCode::OK, "").into_response()
}

pub(crate) async fn dispatch_interactivity(
    payload: InteractivityPayload,
    // The node's Layer-1 pre-aggregation cache, used only by the two actions
    // that dispatch an agent run. A default means those runs compile their
    // semantic queries to warehouse SQL instead of reading a rollup.
    preagg: PreaggCacheCtx,
) {
    tracing::info!(
        actions = ?payload.actions.iter().map(|a| &a.action_id).collect::<Vec<_>>(),
        "interactivity payload received"
    );

    for action in &payload.actions {
        let outcome = match action.action_id.as_str() {
            "slack_pick_workspace" => {
                handlers::pick_workspace::handle(&payload, action, &preagg).await
            }
            "slack_submit_workspace_picker" => {
                handlers::submit_workspace_picker::handle(&payload, action, &preagg).await
            }
            "slack_reopen_picker" => handlers::reopen_picker::handle(&payload, action).await,
            "slack_make_default" => handlers::make_default::handle(&payload, action).await,
            "slack_home_disconnect" => handlers::home_disconnect::handle(&payload).await,
            "slack_home_save_defaults" => handlers::home_save_defaults::handle(&payload).await,
            "slack_view_sql_artifacts" => {
                handlers::view_sql_artifacts::handle(&payload, action).await
            }
            // View state / URL buttons — no server-side action needed.
            "slack_home_pick_workspace"
            | "slack_home_pick_agent"
            | "slack_home_connect"
            | "slack_home_open_oxy"
            | "slack_view_thread"
            | "slack_connect_oxy" => Ok(()),
            other => {
                tracing::info!("unhandled interactivity action: {other}");
                Ok(())
            }
        };
        if let Err(e) = outcome {
            tracing::error!("interactivity action {} failed: {e}", action.action_id);
        }
    }
}
