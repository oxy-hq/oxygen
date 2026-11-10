//! Toast POS reconciliation adapter.
//!
//! Auth (`auth`) exchanges OAuth client-credentials for a short-lived bearer
//! (or falls back to a static token). Reporting (`analytics`) drives Toast's
//! async, batched Analytics Metrics API: one report per window covers every
//! restaurant, and each check reduces that shared report to its number. The
//! pure parse/reduce steps in `analytics` are unit-tested without a network.

mod analytics;
mod auth;

use async_trait::async_trait;

use oxy::config::model::ToastAnalyticsIntegration;

use super::source::{ExternalRequest, ExternalValue, ReconcileError, ReconcileSource, SourceCtx};
use analytics::MetricsRow;
use std::collections::HashMap;

/// Logical secret slots, used as the keys into `SourceCtx::secrets`. The
/// workspace-secret *var names* behind each slot are configurable via the
/// `toast` integration in `config.yml`; these constants are only the in-process
/// labels the auth step reads by.
pub(super) const CLIENT_ID_SECRET: &str = "client_id";
pub(super) const CLIENT_SECRET_SECRET: &str = "client_secret";
pub(super) const API_TOKEN_SECRET: &str = "api_token";

/// Default Toast API gateway when the `toast` integration omits `base_url`.
const DEFAULT_BASE_URL: &str = "https://ws-api.toasttab.com";

pub struct ToastSource {
    http: reqwest::Client,
    base_url: String,
    /// Workspace-secret var names taken from the `toast` integration. There are
    /// no defaults: a slot with no configured var name resolves no secret, so
    /// auth degrades to `NotConfigured` rather than probing a guessed name.
    client_id_var: Option<String>,
    client_secret_var: Option<String>,
    api_token_var: Option<String>,
}

impl ToastSource {
    /// Build from the workspace's `toast_analytics` integration. Secret
    /// var-names come solely from config (no fallback names).
    ///
    /// `base_url` precedence: integration `base_url` → `OXY_TOAST_BASE_URL` env
    /// → prod gateway default. (`api.toasttab.com` is the marketing site and
    /// 404s on API paths; the gateway is `ws-api.toasttab.com` prod /
    /// `ws-sandbox-api.toasttab.com` sandbox.)
    pub fn from_config(toast: Option<&ToastAnalyticsIntegration>) -> Self {
        let base_url = toast
            .and_then(|t| t.base_url.clone())
            .or_else(|| std::env::var("OXY_TOAST_BASE_URL").ok())
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        Self {
            http: oxy_shared::http::client(),
            base_url,
            client_id_var: toast.and_then(|t| t.client_id_var.clone()),
            client_secret_var: toast.and_then(|t| t.client_secret_var.clone()),
            api_token_var: toast.and_then(|t| t.api_token_var.clone()),
        }
    }
}

#[async_trait]
impl ReconcileSource for ToastSource {
    /// Only the slots with a configured var name — an unconfigured slot resolves
    /// no secret.
    fn secret_vars(&self) -> Vec<(&'static str, String)> {
        [
            (CLIENT_ID_SECRET, self.client_id_var.as_ref()),
            (CLIENT_SECRET_SECRET, self.client_secret_var.as_ref()),
            (API_TOKEN_SECRET, self.api_token_var.as_ref()),
        ]
        .into_iter()
        .filter_map(|(slot, var)| var.map(|v| (slot, v.clone())))
        .collect()
    }

    /// One report per distinct window, shared across requests; each request
    /// reduces its window's report to a number. A single auth/report failure
    /// degrades only the affected requests (one result each), never the whole
    /// sweep.
    async fn fetch_externals(
        &self,
        ctx: &SourceCtx,
        requests: &[ExternalRequest<'_>],
    ) -> Vec<Result<ExternalValue, ReconcileError>> {
        // Optional narrowing filter for the report: the union of the GUIDs the
        // requests already name. Empty ⇒ omit it ⇒ Toast covers the whole
        // management group. No separate restaurant config is ever required.
        let restaurant_ids = restaurant_filter(requests);
        let token = match auth::resolve_bearer(&self.http, &self.base_url, ctx).await {
            Ok(t) => t,
            Err(e) => return err_for_all(requests, e),
        };

        let mut reports: HashMap<[String; 2], Result<Vec<MetricsRow>, ReconcileError>> =
            HashMap::new();
        let mut out = Vec::with_capacity(requests.len());
        for req in requests {
            let window = req.window;
            if !reports.contains_key(window) {
                let report = analytics::fetch_report(
                    &self.http,
                    &self.base_url,
                    &token,
                    &restaurant_ids,
                    window,
                    ctx.report_timeout,
                )
                .await;
                reports.insert(window.clone(), report);
            }
            // The grouped value is cut from the same report: one report per
            // window either way, so group_by costs no extra Toast request.
            out.push(match reports.get(window).expect("just inserted") {
                Ok(rows) => analytics::reduce(rows, req.spec, req.group_by),
                Err(e) => Err(e.clone()),
            });
        }
        out
    }
}

/// Optional `restaurantIds` filter for the report: the de-duplicated union of
/// the GUIDs the requests name (`spec.restaurants`). Empty when no request names
/// any — the caller then omits the filter and Toast covers the whole management
/// group.
fn restaurant_filter(requests: &[ExternalRequest<'_>]) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    for req in requests {
        for guid in &req.spec.restaurants {
            if !ids.contains(guid) {
                ids.push(guid.clone());
            }
        }
    }
    ids
}

/// Same error for every request in the batch (auth/config failure before any
/// per-request work).
fn err_for_all(
    requests: &[ExternalRequest<'_>],
    err: ReconcileError,
) -> Vec<Result<ExternalValue, ReconcileError>> {
    requests.iter().map(|_| Err(err.clone())).collect()
}

/// Trim a response body for inclusion in an error message — enough to identify
/// what came back (a JSON error, an HTML page) without dumping kilobytes.
pub(super) fn truncate_body(body: &str) -> String {
    const MAX: usize = 300;
    let trimmed = body.trim();
    if trimmed.chars().count() <= MAX {
        trimmed.to_string()
    } else {
        format!("{}…", trimmed.chars().take(MAX).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::ExternalSpec;
    use super::*;

    fn spec_with_restaurants(restaurants: &[&str]) -> ExternalSpec {
        ExternalSpec {
            source: "toast".to_string(),
            integration: None,
            metric: "net_sales".to_string(),
            restaurants: restaurants.iter().map(|s| s.to_string()).collect(),
            keys: Default::default(),
        }
    }

    static WINDOW: [String; 2] = [String::new(), String::new()];

    fn req<'a>(spec: &'a ExternalSpec) -> ExternalRequest<'a> {
        ExternalRequest {
            spec,
            window: &WINDOW,
            group_by: &[],
        }
    }

    fn toast_integration(
        client_id_var: Option<&str>,
        base_url: Option<&str>,
    ) -> ToastAnalyticsIntegration {
        ToastAnalyticsIntegration {
            client_id_var: client_id_var.map(str::to_string),
            client_secret_var: None,
            api_token_var: None,
            base_url: base_url.map(str::to_string),
        }
    }

    #[test]
    fn from_config_resolves_no_secret_vars_when_integration_absent() {
        // SAFETY: single-threaded test; no other test reads OXY_TOAST_BASE_URL.
        unsafe { std::env::remove_var("OXY_TOAST_BASE_URL") };
        let src = ToastSource::from_config(None);
        // base_url still falls back to the prod gateway default...
        assert_eq!(src.base_url, DEFAULT_BASE_URL);
        // ...but with no integration there are NO secret var-names to resolve,
        // so auth will degrade to NotConfigured.
        assert!(src.secret_vars().is_empty());
    }

    #[test]
    fn from_config_uses_only_configured_var_names() {
        let integ = toast_integration(
            Some("MY_TOAST_ID"),
            Some("https://ws-sandbox-api.toasttab.com"),
        );
        let src = ToastSource::from_config(Some(&integ));
        assert_eq!(src.base_url, "https://ws-sandbox-api.toasttab.com");
        // Only the configured slot is emitted; the omitted client_secret /
        // api_token slots are absent (no guessed default).
        assert_eq!(
            src.secret_vars(),
            vec![(CLIENT_ID_SECRET, "MY_TOAST_ID".to_string())]
        );
    }

    #[test]
    fn filter_unions_config_restaurants() {
        let a = spec_with_restaurants(&["a", "b"]);
        let b = spec_with_restaurants(&["b", "c"]);
        assert_eq!(restaurant_filter(&[req(&a), req(&b)]), vec!["a", "b", "c"]);
    }

    #[test]
    fn filter_empty_for_all_aggregate_config() {
        // No request names a restaurant ⇒ no filter ⇒ whole management group.
        let agg = spec_with_restaurants(&[]);
        assert!(restaurant_filter(&[req(&agg)]).is_empty());
    }
}
