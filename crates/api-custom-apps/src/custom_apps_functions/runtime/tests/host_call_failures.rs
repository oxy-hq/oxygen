//! Host-call failures: what is noted, what is not, and recovery.

use super::*;

/// `ctx.platform.facts` is bound, carries its options to the host as an
/// object (`{}` when called bare), and resolves to the host's reply.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_platform_facts_hands_the_host_its_options() {
    let (result, _host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
              const asked = await ctx.platform.facts({ weeks: 4 });
              const bare = await ctx.platform.facts();
              return Response.json({ asked, bare });
            };
            "#,
    )
    .await;
    let resp = result.expect("handler must complete");
    let parsed: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(parsed["asked"]["echo"], serde_json::json!({ "weeks": 4 }));
    assert_eq!(parsed["bare"]["echo"], serde_json::json!({}));
}

/// The internal app tells a refusal apart by the label inside the thrown
/// error's message, so the message has to arrive as
/// `ctx.platform.facts: <Label>: …`, label intact.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_platform_facts_call_throws_the_label_in_its_message() {
    let host = Arc::new(MockHost {
        platform_facts_error: Some(
            "PlatformFactsNotEnabled: this app's org is not an internal org".into(),
        ),
        ..Default::default()
    });
    let resp = run_on(
        r#"
            export default async (req, ctx) => {
              try { await ctx.platform.facts(); return new Response("resolved"); }
              catch (e) { return Response.json({ name: e.name, message: e.message }); }
            };
            "#,
        test_ctx(),
        host,
    )
    .await
    .expect("the handler caught the refusal and returned");
    let parsed: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
    assert_eq!(parsed["name"], "HostError");
    assert_eq!(
        parsed["message"],
        "ctx.platform.facts: PlatformFactsNotEnabled: this app's org is not an internal org"
    );
}

/// A handler that catches a failed `ctx.*` call and answers 200 is the
/// failure that never paged. The broker notes it on the host before the
/// isolate sees the rejection, so catching it cannot hide it.
const CATCHES_A_FAILED_QUERY: &str = r#"
        export default async (req, ctx) => {
            let seen = "none";
            try { await ctx.query("select 1"); } catch (e) { seen = e.message; }
            return new Response(seen);
        };
    "#;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caught_host_call_failure_is_noted_on_the_host() {
    let host = Arc::new(MockHost {
        query_error: Some("connection refused".into()),
        ..Default::default()
    });
    let resp = run_on(CATCHES_A_FAILED_QUERY, test_ctx(), host.clone())
        .await
        .expect("the handler caught the failure and returned");
    assert_eq!(resp.status, 200);
    assert!(resp.body.contains("connection refused"), "{}", resp.body);
    assert_eq!(
        host.host_call_failures(),
        // `connection refused` is the connection never opening: the kind
        // that names it (`host_call_attrs::is_unreachable`), not the catch-all.
        vec![("query", "unreachable")],
        "exactly one note, for the one failed call"
    );
    assert_eq!(
        host.host_call_failure_messages(),
        vec!["connection refused"],
        "the note carries the host's message, for the fingerprint"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_not_found_host_call_failure_is_not_noted() {
    let host = Arc::new(MockHost {
        query_error: Some("object not found".into()),
        ..Default::default()
    });
    let resp = run_on(CATCHES_A_FAILED_QUERY, test_ctx(), host.clone())
        .await
        .expect("the handler caught the failure and returned");
    // The call did fail — so an empty note list means "not paged", not
    // "never reached the host".
    assert!(resp.body.contains("object not found"), "{}", resp.body);
    assert_eq!(host.host_call_failures(), Vec::new());
}

/// A handler that retries a failed call and gets its answer has recovered.
/// The broker reports the success under the same closed-list op name as
/// the failure, which is what lets the host's note clear
/// (`HostCallFailure::recovered`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_retried_host_call_reports_its_success_under_the_same_op() {
    let host = Arc::new(MockHost {
        query_error: Some("connection refused".into()),
        query_error_once: true,
        ..Default::default()
    });
    let resp = run_on(
        r#"
            export default async (req, ctx) => {
                let rows;
                try { rows = await ctx.query("select 1"); }
                catch (e) { rows = await ctx.query("select 1"); }
                return Response.json({ rows });
            };
        "#,
        test_ctx(),
        host.clone(),
    )
    .await
    .expect("the retry answered");
    assert_eq!(resp.status, 200, "{}", resp.body);
    assert_eq!(host.host_call_failures(), vec![("query", "unreachable")]);
    assert_eq!(host.host_call_successes(), vec!["query"]);
    // `ctx.query` names no target, so the note is keyed by op alone.
    assert_eq!(host.host_call_failure_targets(), vec![None]);
    assert_eq!(
        host.host_call_successes_with_targets(),
        vec![("query", None)]
    );
}

/// The broker names where a call went alongside its op, on failure and on
/// success, so the host can clear a caught failure only for the same
/// target (`HostCallFailure::recovered`): a `fetch`'s host, by the one URL
/// parser; a `warehouse.*` op's database and table; nothing for an op
/// that names no place. Neither the URL's path nor the query reaches the
/// note.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_host_call_is_noted_with_its_target() {
    let host = Arc::new(MockHost::default());
    let resp = run_on(
            r#"
            export default async (req, ctx) => {
                try { await ctx.fetch("https://api.vendor-a.com/v1/rates?key=sk_live_abc"); } catch (e) {}
                try { await ctx.fetch("https://hooks.slack.com/services/T0/B0/x"); } catch (e) {}
                const r = await ctx.warehouse.query("dw", "SELECT 1");
                return Response.json({ rows: r.rows.length });
            };
        "#,
            test_ctx(),
            host.clone(),
        )
        .await
        .expect("the handler caught both fetches and answered");
    assert_eq!(resp.status, 200, "{}", resp.body);
    assert_eq!(
        host.host_call_failures(),
        vec![("fetch", "host_call_failed"), ("fetch", "host_call_failed")]
    );
    assert_eq!(
        host.host_call_failure_targets(),
        vec![
            Some("api.vendor-a.com".to_string()),
            Some("hooks.slack.com".to_string())
        ],
        "two hosts under one op are two targets"
    );
    assert_eq!(
        host.host_call_successes_with_targets(),
        vec![("warehouse.query", Some("dw".to_string()))]
    );
}

/// A refusal of the call's own arguments is the app's error, and the run
/// fails the same way every time; catching it is not hiding a failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_caught_bad_request_is_not_noted() {
    let host = Arc::new(MockHost {
        query_error: Some("`sql` is required".into()),
        ..Default::default()
    });
    let resp = run_on(CATCHES_A_FAILED_QUERY, test_ctx(), host.clone())
        .await
        .expect("the handler caught the refusal and returned");
    assert!(resp.body.contains("`sql` is required"), "{}", resp.body);
    assert_eq!(host.host_call_failures(), Vec::new());
}

/// `ctx.airhouse` and `ctx.oltp.tx` reach the broker like every other host
/// call, so a caught failure in either is noted, and under its own name
/// from the closed list rather than `airhouse.other` or `tx.other`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn caught_airhouse_and_oltp_tx_failures_are_noted_by_name() {
    let host = Arc::new(MockHost {
        own_store_error: Some("connection refused".into()),
        ..Default::default()
    });
    let resp = run_on(
        r#"
            export default async (req, ctx) => {
                const seen = [];
                try {
                    await ctx.airhouse.append("visits", [{ visit_id: "v1" }]);
                } catch (e) { seen.push(e.message); }
                try {
                    await ctx.oltp.tx(async (tx) => tx.exec("UPDATE bookings SET seated = true"));
                } catch (e) { seen.push(e.message); }
                return Response.json({ seen });
            };
        "#,
        test_ctx(),
        host.clone(),
    )
    .await
    .expect("the handler caught both failures and returned");
    assert_eq!(resp.status, 200);
    assert_eq!(
        resp.body.matches("connection refused").count(),
        2,
        "both calls failed: {}",
        resp.body
    );
    assert_eq!(
        host.host_call_failures(),
        vec![
            ("airhouse.append", "unreachable"),
            ("tx.begin_oltp", "unreachable"),
        ]
    );
}
