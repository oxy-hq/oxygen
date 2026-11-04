//! `ctx.tx`, `ctx.warehouse`, `ctx.oltp` and `ctx.airhouse` across the boundary.

use super::*;

/// The happy path of the `ctx.tx` bracket: begin → the author's statements
/// → commit, with the callback's return value handed back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_tx_commits_when_the_callback_resolves() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const id = await ctx.tx("appdb", async (tx) => {
                    const rows = await tx.query("INSERT INTO orders DEFAULT VALUES RETURNING id");
                    await tx.exec("UPDATE inventory SET on_hand = on_hand - $1", [2]);
                    return rows[0].id;
                });
                return Response.json({ id });
            };
        "#,
    )
    .await;

    let resp = result.expect("ctx.tx must resolve");
    assert!(resp.body.contains(r#""id":42"#), "{}", resp.body);
    assert_eq!(
        host.tx_ops(),
        vec!["begin", "query", "exec", "commit"],
        "the wrapper must commit exactly once, after the author's statements"
    );
}

/// The property the whole bracket exists for: an author who throws — or
/// whose statement fails — must not leave a transaction open, and must
/// still see their own error rather than a rollback error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_tx_rolls_back_when_the_callback_throws_and_rethrows_the_original() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                let seen = null;
                try {
                    await ctx.tx("appdb", async (tx) => {
                        await tx.exec("INSERT INTO orders DEFAULT VALUES");
                        throw new Error("inventory went negative");
                    });
                } catch (e) {
                    seen = e.message;
                }
                return Response.json({ seen });
            };
        "#,
    )
    .await;

    let resp = result.expect("the handler itself must still return");
    assert!(
        resp.body.contains("inventory went negative"),
        "the author's error must survive the rollback: {}",
        resp.body
    );
    assert_eq!(
        host.tx_ops(),
        vec!["begin", "exec", "rollback"],
        "a throwing callback must roll back and must NOT commit"
    );
}

/// A handle that escapes its callback must fail loudly. Without this, a
/// stashed handle would address whatever transaction later holds that id.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tx_handle_used_after_the_callback_returns_is_rejected() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                let escaped;
                await ctx.tx("appdb", async (tx) => { escaped = tx; });
                let threw = false;
                try { await escaped.exec("DELETE FROM orders"); } catch (e) { threw = true; }
                return Response.json({ threw });
            };
        "#,
    )
    .await;

    let resp = result.expect("handler must return");
    assert!(resp.body.contains(r#""threw":true"#), "{}", resp.body);
    assert_eq!(
        host.tx_ops(),
        vec!["begin", "commit"],
        "the escaped handle must never reach the host"
    );
}

/// Parameters must cross the boundary as a JSON array. If they arrived
/// stringified, binding would silently become interpolation — the exact
/// failure this API exists to prevent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tx_params_cross_the_boundary_as_an_array() {
    let (result, _host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const rows = await ctx.tx("appdb", (tx) =>
                    tx.query("SELECT * FROM t WHERE a = $1 AND b = $2", [7, "x"]));
                return Response.json({ params: rows[0].params });
            };
        "#,
    )
    .await;

    let resp = result.expect("handler must return");
    assert!(
        resp.body.contains(r#""params":[7,"x"]"#),
        "params must arrive as an array: {}",
        resp.body
    );
}

/// Omitting `params` is the common case (a statement with no placeholders)
/// and must not become `undefined` on the wire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tx_params_default_to_an_empty_array() {
    let (result, _host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const rows = await ctx.tx("appdb", (tx) => tx.query("SELECT 1"));
                return Response.json({ params: rows[0].params });
            };
        "#,
    )
    .await;

    let resp = result.expect("handler must return");
    assert!(resp.body.contains(r#""params":[]"#), "{}", resp.body);
}

/// Regression: a handler that awaits an async host op (`ctx.query`) must
/// RESUME and return once the op replies — it must not hang until the
/// wall-clock timeout. This reproduces the `resolve()`-doesn't-pump-the-
/// event-loop bug: with the buggy invoke path this returns `Err(Timeout)`;
/// with `with_event_loop_promise` it returns the handler's `Response` fast.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handler_resumes_after_async_host_op() {
    let artifact = r#"
            export default async (req, ctx) => {
                const r = await ctx.query("select 1");
                return Response.json({ rows: r.rows.length });
            };
        "#;
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let result = run(
        artifact.to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        Arc::new(MockHost::default()),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await;
    let resp = result.expect("handler must resume after the async host op, not time out");
    assert!(
        resp.body.contains("\"rows\":1"),
        "unexpected handler body: {}",
        resp.body
    );
}

/// `ctx.warehouse.query` reaches a named database.
///
/// `ctx.query` only ever hits the project default, and every other
/// named-database surface is a write — so before this an app had no way to
/// read its own OLTP store unless it happened to be the default.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warehouse_query_reads_a_named_database() {
    let artifact = r#"
            export default async (req, ctx) => {
                const r = await ctx.warehouse.query("oltp", "SELECT 1");
                return Response.json({ rows: r.rows.length, db: r.db });
            };
        "#;
    let host = Arc::new(MockHost::default());
    let (_cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let resp = run(
        artifact.to_string(),
        uuid::Uuid::new_v4(),
        test_ctx(),
        FnRequest::from_body(b"{}".to_vec()),
        host.clone(),
        cancel_rx,
        std::time::Duration::from_secs(10),
        Arc::new(std::sync::Mutex::new(Vec::new())),
        InvocationMeters::start(),
        tracing::Span::none(),
    )
    .await
    .expect("handler should succeed");

    assert!(resp.body.contains("\"rows\":1"), "body: {}", resp.body);
    // The database name has to survive the boundary, or the read would
    // silently target whatever the host defaulted to.
    assert!(resp.body.contains("\"db\":\"oltp\""), "body: {}", resp.body);
}

/// `ctx.oltp.query(sql, params)` reaches `host.oltp("query", { sql, params })`
/// with `params` as an array — the whole JS → op → HostCall → dispatch wiring
/// for the app-writer path. No database name crosses the boundary: the app's
/// own writer is derived host-side from its slug (the manifest only gates).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oltp_query_forwards_sql_and_params_to_the_host() {
    let (result, _host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const rows = await ctx.oltp.query(
                    "SELECT * FROM bookings WHERE party_size > $1", [4]);
                return Response.json(rows[0]);
            };
        "#,
    )
    .await;

    let resp = result.expect("handler must return");
    assert!(
        resp.body.contains(r#""params":[4]"#),
        "params must cross as an array, not a stringified blob: {}",
        resp.body
    );
    assert!(
        resp.body.contains("party_size"),
        "sql must cross the boundary intact: {}",
        resp.body
    );
}

/// `ctx.oltp.tx(fn)` opens with `begin_oltp` — no database crosses the
/// boundary, the writer is the host's to derive — and closes through the
/// same commit bracket as `ctx.tx`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oltp_tx_opens_on_the_apps_own_store_and_commits() {
    let (result, host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const n = await ctx.oltp.tx(async (tx) => {
                    await tx.exec("UPDATE bookings SET seated = true WHERE id = $1", [7]);
                    return tx.exec("INSERT INTO seatings (booking_id) VALUES ($1)", [7]);
                });
                return Response.json({ n });
            };
        "#,
    )
    .await;

    let resp = result.expect("ctx.oltp.tx must resolve");
    assert!(resp.body.contains(r#""n":1"#), "body: {}", resp.body);
    assert_eq!(host.tx_ops(), vec!["begin_oltp", "exec", "exec", "commit"]);
}

/// `ctx.airhouse.append(table, rows)` sends a bare table and the rows as an
/// array; adding the schema is the host's job, not the script's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn airhouse_append_forwards_the_table_and_rows() {
    let (result, _host) = run_with_mock(
            r#"
            export default async (req, ctx) => {
                const n = await ctx.airhouse.append("visits", [{ visit_id: "v1" }, { visit_id: "v2" }]);
                return Response.json({ n, schema: ctx.airhouse.schema });
            };
        "#,
        )
        .await;

    let resp = result.expect("ctx.airhouse.append must resolve");
    assert!(resp.body.contains(r#""n":2"#), "body: {}", resp.body);
    assert!(
        resp.body.contains(r#""schema":null"#),
        "no capability, no schema: {}",
        resp.body
    );
}

/// `ctx.oltp.exec` omitting `params` sends `[]`, not `undefined` — the same
/// guarantee `ctx.tx` makes, so a no-placeholder write is not read as a
/// wrong-arity error host-side.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oltp_exec_returns_row_count() {
    let (result, _host) = run_with_mock(
        r#"
            export default async (req, ctx) => {
                const n = await ctx.oltp.exec("DELETE FROM bookings WHERE cancelled");
                return Response.json({ n });
            };
        "#,
    )
    .await;

    let resp = result.expect("handler must return");
    assert!(resp.body.contains(r#""n":1"#), "body: {}", resp.body);
}
