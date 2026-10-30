//! The All Quiet copy of the function-failure page: a Minor incident that asks
//! ferret for a priority verdict (`Triage = ferret`).
//!
//! The Slack page alone reached a channel nobody is named to read: on
//! 2026-10-28 it was the most specific signal of the Airhouse empty-result P0
//! (`bookkeeping/post-je`) and nobody acted on it. As an All Quiet incident it
//! is seen by ferret's poller, which reads the error text itself from
//! `GET /api/customer-apps/function-failures` with the ids below.
//!
//! Sent beside the Slack post, under the same claim — so at most as often —
//! and only when [`WEBHOOK_URL_VAR`] is set. Best effort: the claim's delivery
//! follows the Slack post, and a Slack retry re-sends this too, which All Quiet
//! folds into the same incident by `correlation_id`. Never a page: the
//! integration's routing rule pins it to Minor whatever this sends
//! (infrastructure `allquiet/uptime/function-failures.tf`). Nothing resolves
//! the incident; a person does.
//!
//! The attribute names are a contract with ferret (`src/allquiet/poller.ts`,
//! `src/triage/`), agreed 2026-10-31; the integration's mapping copies each
//! field below to the attribute named beside it.

use chrono::{DateTime, SecondsFormat, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

/// The All Quiet webhook integration's URL, a secret: whoever holds it can
/// open incidents. Unset or blank, nothing is sent.
const WEBHOOK_URL_VAR: &str = "OXY_OPS_ALLQUIET_FUNCTION_FAILURES_URL";

/// What the incident says. There is no field for the failure's message, on
/// purpose: the Slack page leaves it out for the same reason (`failure_page`).
#[derive(Debug, Clone, Copy)]
pub(super) struct Incident<'a> {
    pub org_id: Uuid,
    /// `None` when the org could not be read; the incident still goes out.
    pub org_slug: Option<&'a str>,
    pub app_id: Uuid,
    pub app_slug: &'a str,
    pub function: &'a str,
    pub kind: &'static str,
    pub fingerprint: &'a str,
    pub invocation_id: Uuid,
    pub first_seen: DateTime<Utc>,
}

/// The webhook body.
pub(super) fn payload(i: &Incident<'_>) -> Value {
    json!({
        "status": "Open",                                    // Status
        "title": format!(                                    // Title
            "Custom-app function failing: {}/{}", i.app_slug, i.function
        ),
        "correlation_id": format!(                           // CorrelationId
            "custom-app-function:{}:{}:{}", i.app_id, i.function, i.fingerprint
        ),
        "triage": "ferret",                                  // Triage
        "source": "custom-app-function",                     // Source
        "org_id": i.org_id,                                  // OrgId
        "org_slug": i.org_slug,                              // OrgSlug
        "app_id": i.app_id,                                  // AppId
        "app_slug": i.app_slug,                              // AppSlug
        "function": i.function,                              // Function
        "kind": i.kind,                                      // Kind
        "fingerprint": i.fingerprint,                        // Fingerprint
        "invocation_id": i.invocation_id,                    // InvocationId
        "first_seen": i.first_seen.to_rfc3339_opts(SecondsFormat::Secs, true), // FirstSeen
    })
}

pub(super) fn webhook_url() -> Option<String> {
    webhook_url_from(|name| std::env::var(name).ok())
}

fn webhook_url_from(var: impl Fn(&str) -> Option<String>) -> Option<String> {
    var(WEBHOOK_URL_VAR).filter(|v| !v.trim().is_empty())
}

/// POST `body` to `url` within `timeout`. The error never carries the URL.
pub(super) async fn post(
    url: &str,
    body: &Value,
    timeout: std::time::Duration,
) -> Result<(), String> {
    let sent = reqwest::Client::new()
        .post(url)
        .timeout(timeout)
        .json(body)
        .send()
        .await
        .and_then(|r| r.error_for_status());
    sent.map(|_| ()).map_err(|e| e.without_url().to_string())
}

#[cfg(test)]
mod tests {
    use super::super::failure_signal::{Failure, HostCallFailure};
    use super::*;

    fn incident<'a>(failure: &'a Failure, org_slug: Option<&'a str>) -> Incident<'a> {
        Incident {
            org_id: Uuid::from_u128(1),
            org_slug,
            app_id: Uuid::from_u128(2),
            app_slug: "bookkeeping",
            function: "post-je",
            kind: failure.kind,
            fingerprint: &failure.fingerprint,
            invocation_id: Uuid::from_u128(3),
            first_seen: DateTime::from_timestamp(1_793_180_000, 0).unwrap(),
        }
    }

    #[test]
    fn the_page_builds_the_all_quiet_payload_ferret_triages() {
        let threw = Failure::of(
            "error",
            0,
            Some("function threw: ctx.warehouse: failed to convert query results"),
            None,
            None,
        )
        .unwrap();
        let body = payload(&incident(&threw, Some("acme")));

        assert_eq!(body["triage"], "ferret");
        assert_eq!(body["source"], "custom-app-function");
        assert_eq!(body["status"], "Open");
        assert_eq!(
            body["title"],
            "Custom-app function failing: bookkeeping/post-je"
        );
        assert_eq!(body["org_id"], Uuid::from_u128(1).to_string());
        assert_eq!(body["org_slug"], "acme");
        assert_eq!(body["app_id"], Uuid::from_u128(2).to_string());
        assert_eq!(body["app_slug"], "bookkeeping");
        assert_eq!(body["function"], "post-je");
        assert_eq!(body["kind"], threw.kind);
        assert_eq!(body["fingerprint"], threw.fingerprint.as_str());
        assert_eq!(body["invocation_id"], Uuid::from_u128(3).to_string());
        assert_eq!(body["first_seen"], "2026-10-28T09:33:20Z");
        assert_eq!(
            body["correlation_id"],
            format!(
                "custom-app-function:{}:post-je:{}",
                Uuid::from_u128(2),
                threw.fingerprint
            )
        );
        // ferret's poller would read a severity as the incident's; the
        // integration pins Minor, so the body names none.
        assert!(body.get("severity").is_none(), "{body}");
    }

    /// The Slack page names the failure's shape, never its message, and so does
    /// this: the tenant's error text stays where reading it takes a staff key.
    #[test]
    fn the_all_quiet_payload_carries_no_error_text() {
        let threw = Failure::of(
            "error",
            0,
            Some("function threw: Error: customer 4412 owes $9,310"),
            None,
            None,
        )
        .unwrap();
        let hc = HostCallFailure {
            op: "warehouse.insert",
            kind: "host_call_failed",
            message: "warehouse insert failed: HTTP 400 Bad Request: Code: 27".into(),
            target: None,
        };
        let caught = Failure::of("success", 200, None, None, Some(&hc)).unwrap();

        for (failure, message) in [(&threw, "customer 4412 owes"), (&caught, "Bad Request")] {
            let text = payload(&incident(failure, None)).to_string();
            assert!(!text.contains(message), "{text}");
        }
    }

    #[test]
    fn an_unknown_org_slug_is_sent_as_null() {
        let threw = Failure::of("error", 0, Some("function threw: x"), None, None).unwrap();
        assert!(payload(&incident(&threw, None))["org_slug"].is_null());
    }

    #[test]
    fn no_url_or_a_blank_one_sends_nothing() {
        assert_eq!(webhook_url_from(|_| None), None);
        assert_eq!(webhook_url_from(|_| Some("  ".into())), None);
        assert_eq!(
            webhook_url_from(|n| (n == WEBHOOK_URL_VAR).then(|| "https://aq/x".into())),
            Some("https://aq/x".into())
        );
    }
}
