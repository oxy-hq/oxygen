//! Pure helpers behind the per-host-op spans in `runtime::run` — what a
//! `ctx.query` / `ctx.fetch` / … is allowed to say about itself on the
//! platform trace.
//!
//! The rule for every attribute here: shape, never payload. A query summary
//! is a verb and a table, not the SQL (which can embed literals from user
//! input); a fetch target is a scheme, host and port, not the URL (which is
//! where API keys travel as query strings). The tenant's own store already
//! holds the content behind the app-admin gate; the operator store gets the
//! timing and the type of failure.

/// The `tracing` target of every host-op span. Platform-only: the product
/// `SpanCollectorLayer` collects every span at its level with no name filter,
/// so without this a function looping 200 queries would write 200 span rows
/// per invocation into the tenant-facing store — a store the docs describe as
/// "agent/automation spans only". `oxy_observability::observability_filter`
/// switches this target off; the OTLP trace layer keeps it.
pub(super) const HOST_CALL_TARGET: &str = "oxy::host_call";

/// A token that can be a table or verb name, bounded — anything else (a
/// 4 KB expression, an unquoted fragment) is not recorded at all. This is the
/// guarantee "never the SQL" rests on: whatever reaches a span is one
/// whitespace-delimited, identifier-shaped token of at most 64 chars, taken
/// after [`strip_string_literals`] has removed standard single-quoted
/// literals. It is not a dialect-aware lexer — a backslash-escaped quote or a
/// double-quoted literal can still end a literal early — so the bound, not
/// the stripping, is what holds.
pub(super) fn identifier_like(token: &str) -> bool {
    !token.is_empty()
        && token.len() <= 64
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '$' | '-'))
}

/// The first verb and the first table of a SQL text, for `db.operation.name`
/// and `db.collection.name`. Best-effort on a whitespace scan — a CTE reads
/// as `WITH` with no table, which is honest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct QuerySummary {
    pub verb: String,
    pub table: String,
}

/// `sql` past any leading whitespace, `-- …` line comments and `/* … */` block
/// comments, so the verb is the statement's first keyword rather than `--`. An
/// unterminated block comment leaves nothing.
fn skip_leading_comments(sql: &str) -> &str {
    let mut rest = sql.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, next)| next);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, next)| next);
        } else {
            return rest;
        }
        rest = rest.trim_start();
    }
}

/// Replace every single-quoted string literal (`''` escapes included) with a
/// space, so a keyword inside a literal — `select 'x from secret' …` — is
/// never mistaken for the real one. Unterminated literals swallow the rest.
pub(super) fn strip_string_literals(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\'' {
            out.push(c);
            continue;
        }
        out.push(' ');
        loop {
            match chars.next() {
                None => return out,
                Some('\'') if chars.peek() == Some(&'\'') => {
                    chars.next();
                }
                Some('\'') => break,
                Some(_) => {}
            }
        }
    }
    out
}

pub(super) fn db_query_summary(sql: &str) -> QuerySummary {
    let sql = strip_string_literals(skip_leading_comments(sql));
    let tokens: Vec<&str> = sql
        .split(|c: char| c.is_whitespace() || c == '(' || c == ')' || c == ';' || c == ',')
        .filter(|t| !t.is_empty())
        .collect();
    let verb = tokens
        .first()
        .map(|t| t.to_ascii_uppercase())
        .unwrap_or_default();
    let after = |keyword: &str| -> Option<String> {
        tokens
            .iter()
            .position(|t| t.eq_ignore_ascii_case(keyword))
            .and_then(|i| tokens.get(i + 1))
            .map(|t| t.trim_matches(|c| c == '`' || c == '"').to_string())
    };
    let table = match verb.as_str() {
        "SELECT" | "DELETE" => after("FROM"),
        "INSERT" | "REPLACE" => after("INTO"),
        "UPDATE" => tokens.get(1).map(|t| t.to_string()),
        "CREATE" | "DROP" | "ALTER" | "TRUNCATE" => after("TABLE"),
        _ => None,
    }
    .unwrap_or_default();
    let keep = |s: String| {
        if identifier_like(&s) {
            s
        } else {
            String::new()
        }
    };
    QuerySummary {
        verb: keep(verb),
        table: keep(table),
    }
}

/// Where a `ctx.fetch` goes — never the path or query. The parser lives in
/// `url_shape`, which compiles in every configuration, because the failure
/// fingerprint reads a URL's host by the same rule and this module is gated
/// with the runtime. Handed on from here so a span's attributes still come
/// from one place.
pub(super) use super::url_shape::fetch_target;

/// `error.type` for a failed host op, from the message the host returned.
/// Coarse on purpose: these become a HyperDX facet, and a facet with one
/// value per distinct message is no facet.
///
/// `bad_request` is decided first: the host composes those messages itself,
/// and some of them echo a value the caller sent (`unknown op '<op>'`,
/// `unknown encoding '<encoding>'`), which must not get to pick the kind.
pub(super) fn classify_host_error(message: &str) -> &'static str {
    let m = message.to_ascii_lowercase();
    if is_caller_error(&m) {
        "bad_request"
    } else if m.contains("timed out") || m.contains("timeout") || m.contains("deadline") {
        "timeout"
    } else if m.contains("permission") || m.contains("denied") || m.contains("forbidden") {
        "permission_denied"
    } else if m.contains("not allowed") || m.contains("blocked") || m.contains("capability") {
        "not_allowed"
    } else if m.contains("not found") || m.contains("does not exist") || m.contains("no such") {
        "not_found"
    } else if m.contains("cancel") {
        "cancelled"
    } else {
        "host_call_failed"
    }
}

/// The host refused the call's arguments before doing anything with them: a
/// field missing or of the wrong shape, an op or encoding off its list, a
/// `storage.copy` onto a key that exists without `allowOverwrite`. The app
/// sent that, so it is the app's to fix, and it fails the same way on every
/// call — an app using `AlreadyExists` as its put-if-absent idiom would
/// otherwise clear the paging threshold by itself.
///
/// The same holds for an op the destination's engine cannot do at all:
/// `warehouse.upsert` on a warehouse with no `ON CONFLICT`
/// (`upsert_support::check`), `ctx.tx` on one with no transactions (the
/// connector's `transaction::unsupported`). Both are refused by name before a
/// statement is sent, on every call, by the app's choice of destination —
/// deterministic given the connector the destination resolved to. That
/// resolution is platform state, read from the compiled workspace, so these
/// two markers are the one place this list can swallow a platform fault: a
/// regression that resolved a Postgres destination to a non-transactional
/// connector would fail every `ctx.tx` with this message and page nobody. The
/// trade is taken because the platform canary pins both refusals on its
/// ClickHouse destination every five minutes, and the alternative is a
/// guaranteed page for its own contract check on every run.
///
/// The host returns errors as plain strings, with no caller-vs-platform
/// distinction to read, so this matches the host's own phrasing (`host.rs`,
/// `host/airhouse_ops.rs`, `StorageError`'s `Display`): one marker per shape,
/// each as the host writes it. The backtick in `` ` is required `` and
/// `` ` must be `` is the host quoting a field name; a warehouse's own
/// "is required" or "must be" carries none and stays platform-side.
/// `m` is the lowercased message.
fn is_caller_error(m: &str) -> bool {
    const MARKERS: &[&str] = &[
        "` is required",
        "` must be",
        "each row must be an object",
        "row missing column '",
        "unknown op '",
        "unknown encoding '",
        "unknown bodyencoding '",
        "is not valid base64",
        "invalid url:",
        "invalid method '",
        "invalid semantic query spec:",
        "invalid storage request:",
        "storage conflict:",
        "warehouse.upsert is not supported on",
        "does not support multi-statement transactions",
    ];
    MARKERS.iter().any(|marker| m.contains(marker))
}

/// Whether a [`classify_host_error`] kind pages even when the handler catches
/// it. Not every counted kind is the platform failing. Some are the app's or
/// its config's own condition:
/// - `permission_denied` from a scheduled or manual airhouse write, which
///   holds only a Reader credential;
/// - `not_allowed` from a host missing in `allow_hosts`;
/// - `timeout` from a third party slow to answer `ctx.fetch`.
///
/// Those pages are accepted noise. The new-in-a-week rule and the
/// per-function and hourly caps in `failure_alert` bound them.
/// `not_found` is ordinary control flow (a `storage.head` probe for an object
/// not written yet), `bad_request` is the app's own argument error
/// ([`is_caller_error`]), and `cancelled` is someone asking the run to stop.
pub(super) fn counts_toward_paging(kind: &str) -> bool {
    matches!(
        kind,
        "host_call_failed" | "timeout" | "permission_denied" | "not_allowed"
    )
}

/// Every fixed name a host op pages under: what [`host_op_name`] returns for
/// the families with sub-ops, and the literal `runtime::host_call_span` hands
/// back for each op with one. The unit tests below pin the list to both
/// sources, so a new host op cannot be named without appearing here.
///
/// This is the list `tests/custom_apps/canary_coverage.rs` reads: every name
/// on it is exercised by a step of the platform canary
/// (`customer-apps/examples/platform-canary`) or exempted there with a reason.
/// A name added here without either fails that test.
pub const HOST_OPS: &[&str] = &[
    "query",
    "query_stream",
    "fetch",
    "semantic.query",
    "airway.run",
    "warehouse.insert",
    "warehouse.exec",
    "warehouse.upsert",
    "warehouse.query",
    "tx.begin",
    "tx.begin_oltp",
    "tx.query",
    "tx.exec",
    "tx.commit",
    "tx.rollback",
    "oltp.query",
    "oltp.exec",
    "airhouse.query",
    "airhouse.exec",
    "airhouse.append",
    "storage.getUploadUrl",
    "storage.getDownloadUrl",
    "storage.put",
    "storage.get",
    "storage.head",
    "storage.list",
    "storage.delete",
    "storage.copy",
    "secrets.set",
    "email.send",
    "org.people",
    "org.places",
    "org.assignments",
];

/// The fixed name a host op pages under: its family and sub-op from a closed
/// list, e.g. `warehouse.insert`. The sub-op string arrives from the isolate,
/// so one off the list is `<family>.other`, never the string itself — a page
/// fingerprint is shared across apps and carries nothing an app chose.
/// Every name returned here other than `other` / `<family>.other` is in
/// [`HOST_OPS`].
pub(super) fn host_op_name(family: &str, op: &str) -> &'static str {
    match (family, op) {
        ("warehouse", "insert") => "warehouse.insert",
        ("warehouse", "exec") => "warehouse.exec",
        ("warehouse", "upsert") => "warehouse.upsert",
        ("warehouse", "query") => "warehouse.query",
        ("warehouse", _) => "warehouse.other",
        ("tx", "begin") => "tx.begin",
        ("tx", "begin_oltp") => "tx.begin_oltp",
        ("tx", "query") => "tx.query",
        ("tx", "exec") => "tx.exec",
        ("tx", "commit") => "tx.commit",
        ("tx", "rollback") => "tx.rollback",
        ("tx", _) => "tx.other",
        ("oltp", "query") => "oltp.query",
        ("oltp", "exec") => "oltp.exec",
        ("oltp", _) => "oltp.other",
        ("airhouse", "query") => "airhouse.query",
        ("airhouse", "exec") => "airhouse.exec",
        ("airhouse", "append") => "airhouse.append",
        ("airhouse", _) => "airhouse.other",
        ("storage", "getUploadUrl") => "storage.getUploadUrl",
        ("storage", "getDownloadUrl") => "storage.getDownloadUrl",
        ("storage", "put") => "storage.put",
        ("storage", "get") => "storage.get",
        ("storage", "head") => "storage.head",
        ("storage", "list") => "storage.list",
        ("storage", "delete") => "storage.delete",
        ("storage", "copy") => "storage.copy",
        ("storage", _) => "storage.other",
        _ => "other",
    }
}

/// Row count and truncation flag from a `ctx.query` reply, when the reply
/// has the `{ rows: [...], truncated: bool }` shape.
pub(super) fn rows_and_truncated(value: &serde_json::Value) -> (Option<u64>, Option<bool>) {
    let rows = value
        .get("rows")
        .and_then(|r| r.as_array())
        .map(|r| r.len() as u64);
    let truncated = value.get("truncated").and_then(|t| t.as_bool());
    (rows, truncated)
}

/// The upstream status from a `ctx.fetch` reply.
pub(super) fn fetch_status(value: &serde_json::Value) -> Option<u64> {
    value.get("status").and_then(|s| s.as_u64())
}

/// The FaaS semconv trigger for an invocation `mode`.
pub(super) fn faas_trigger(mode: &str) -> &'static str {
    match mode {
        "route" => "http",
        "schedule" => "timer",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn query_summary_is_verb_and_table_never_the_text() {
        let s = db_query_summary("select a, b from orders where id = 'secret-42'");
        assert_eq!(s.verb, "SELECT");
        assert_eq!(s.table, "orders");
        assert_eq!(
            db_query_summary("INSERT INTO `ledger` (x) VALUES (1)").table,
            "ledger"
        );
        assert_eq!(
            db_query_summary("update Accounts set x = 1").table,
            "Accounts"
        );
        assert_eq!(db_query_summary("delete from t;").table, "t");
        let cte = db_query_summary("with x as (select 1) select * from x");
        assert_eq!(cte.verb, "WITH");
        assert_eq!(cte.table, "");
        assert_eq!(db_query_summary("").verb, "");
    }

    #[test]
    fn a_leading_comment_is_not_the_verb() {
        // The verb decides whether a write is audited (`is_write_verb`), so a
        // comment-prefixed INSERT read as verb `--` was a write with no row.
        let line = db_query_summary("-- receiving report\nINSERT INTO receipts (a) VALUES (1)");
        assert_eq!(
            (line.verb.as_str(), line.table.as_str()),
            ("INSERT", "receipts")
        );
        let block = db_query_summary("  /* nightly */ /* two */\n update ledger set x = 1");
        assert_eq!(
            (block.verb.as_str(), block.table.as_str()),
            ("UPDATE", "ledger")
        );
        assert_eq!(db_query_summary("-- only a comment").verb, "");
        assert_eq!(db_query_summary("/* unterminated insert into t").verb, "");
    }

    #[test]
    fn a_literal_after_from_is_dropped_not_recorded() {
        // The first FROM is inside a string literal in the select list: with
        // literals stripped first, the real FROM is the one that is read.
        let s = db_query_summary("select 'x from secret-42' , a from t");
        assert_eq!(s.verb, "SELECT");
        assert_eq!(s.table, "t");
        // Whitespace before the closing quote would have made `secret42` an
        // identifier-shaped token; stripping the literal removes the chance.
        assert_eq!(
            db_query_summary("select 'x from secret42 ' , a from t").table,
            "t"
        );
        assert_eq!(
            db_query_summary("select 'it''s from here' from t2").table,
            "t2"
        );
        assert_eq!(db_query_summary("select 'unterminated from x").table, "");
        assert_eq!(strip_string_literals("a 'b''c' d"), "a   d");
        let long = format!("select * from {}", "t".repeat(65));
        assert_eq!(db_query_summary(&long).table, "", "bounded at 64");
        assert_eq!(
            db_query_summary("'x' from t").verb,
            "FROM",
            "the literal contributes nothing; the next token is the verb"
        );
        assert!(identifier_like("orders_v2.$tmp-1"));
        assert!(!identifier_like("secret-42'"));
    }

    #[test]
    fn host_errors_classify_coarsely() {
        assert_eq!(classify_host_error("query timed out after 30s"), "timeout");
        assert_eq!(
            classify_host_error("permission denied for table x"),
            "permission_denied"
        );
        assert_eq!(
            classify_host_error("host not allowed by allow_hosts"),
            "not_allowed"
        );
        assert_eq!(
            classify_host_error("relation \"x\" does not exist"),
            "not_found"
        );
        assert_eq!(classify_host_error("something odd"), "host_call_failed");
    }

    #[test]
    fn only_platform_side_host_errors_count_toward_paging() {
        for kind in [
            "host_call_failed",
            "timeout",
            "permission_denied",
            "not_allowed",
        ] {
            assert!(counts_toward_paging(kind), "{kind}");
        }
        // `not_found` is ordinary control flow (a `storage.head` probe), a
        // `bad_request` is the app's own argument error, and a cancellation is
        // someone asking the run to stop.
        for kind in ["not_found", "bad_request", "cancelled"] {
            assert!(!counts_toward_paging(kind), "{kind}");
        }
    }

    /// One message per shape the host refuses a call's arguments with, each as
    /// `host.rs`, `host/airhouse_ops.rs` or `StorageError`'s `Display` writes
    /// it. Dropping a marker from `is_caller_error` fails its line here.
    #[test]
    fn a_host_refusing_the_calls_arguments_is_a_bad_request() {
        for message in [
            "`sql` is required",
            "warehouse.exec: `sql` is required",
            "ctx.storage.put: `body` is required",
            "`table` is required",
            "`params` must be an array of values, got an object. \
             Pass positional arguments for $1, $2, … — e.g. [tableNo, sku].",
            "warehouse.upsert: `conflictColumns` must be a non-empty array",
            "`rows` must be a non-empty array",
            "`table` must be a bare lowercase table name such as \"visits\"",
            "each row must be an object",
            "row missing column 'sku'",
            "unknown op 'drop'",
            "ctx.storage: unknown op 'move'",
            "ctx.fetch: unknown encoding 'latin1' (expected 'utf8' or 'base64')",
            "ctx.fetch: unknown bodyEncoding 'hex' (expected 'utf8' or 'base64')",
            "ctx.storage.put: `body` is not valid base64: Invalid padding",
            "ctx.fetch: `body` is not valid base64: Invalid symbol 32, offset 4.",
            "invalid url: relative URL without a base",
            "invalid method 'FETCH'",
            "invalid semantic query spec: missing field `measures`",
            "invalid storage request: copy source and destination are the same key",
            "storage conflict: 'customer-app-storage/7c1e/generated/report.csv' already \
             exists; pass allowOverwrite to replace it",
        ] {
            assert_eq!(classify_host_error(message), "bad_request", "{message}");
        }
    }

    /// A value the caller sent, echoed inside a refusal, does not pick the kind.
    #[test]
    fn a_caller_value_echoed_in_a_refusal_does_not_reclassify_it() {
        assert_eq!(classify_host_error("unknown op 'timeout'"), "bad_request");
        assert_eq!(
            classify_host_error(
                "ctx.storage.get: unknown encoding 'permission denied' (expected 'utf8' or 'base64')"
            ),
            "bad_request"
        );
    }

    /// The markers are the host's phrasing. A warehouse saying "required" or
    /// "must be" about the app's SQL is still a failed call on the platform's
    /// side of the line, and pages as before.
    #[test]
    fn a_warehouse_message_in_similar_words_is_not_a_bad_request() {
        for message in [
            "ERROR: a column definition list is required for functions returning \"record\"",
            "argument of WHERE must be type boolean, not type integer",
            "Code: 62. DB::Exception: Syntax error: failed at position 8",
        ] {
            assert_eq!(
                classify_host_error(message),
                "host_call_failed",
                "{message}"
            );
        }
    }

    #[test]
    fn a_host_op_name_comes_from_a_closed_list() {
        assert_eq!(host_op_name("warehouse", "insert"), "warehouse.insert");
        assert_eq!(host_op_name("storage", "head"), "storage.head");
        // The sub-op comes from the isolate: off the list, it is not echoed.
        assert_eq!(
            host_op_name("warehouse", "select * from secrets"),
            "warehouse.other"
        );
        assert_eq!(host_op_name("nope", "insert"), "other");
    }

    #[test]
    fn the_airhouse_and_oltp_tx_ops_page_under_their_own_names() {
        for (family, op, name) in [
            ("airhouse", "query", "airhouse.query"),
            ("airhouse", "exec", "airhouse.exec"),
            ("airhouse", "append", "airhouse.append"),
            // `ctx.oltp.tx` opens through the `ctx.tx` op; its later verbs
            // share the `tx.*` names with `ctx.tx(database, fn)`.
            ("tx", "begin_oltp", "tx.begin_oltp"),
        ] {
            assert_eq!(host_op_name(family, op), name, "{family} {op}");
        }
        assert_eq!(host_op_name("airhouse", "drop"), "airhouse.other");
    }

    #[test]
    fn reply_shapes_are_read_defensively() {
        let v = serde_json::json!({ "rows": [1, 2, 3], "truncated": true });
        assert_eq!(rows_and_truncated(&v), (Some(3), Some(true)));
        assert_eq!(rows_and_truncated(&serde_json::json!("nope")), (None, None));
        assert_eq!(
            fetch_status(&serde_json::json!({ "status": 502 })),
            Some(502)
        );
        assert_eq!(fetch_status(&serde_json::json!({})), None);
    }

    #[test]
    fn faas_trigger_follows_the_invocation_mode() {
        assert_eq!(faas_trigger("route"), "http");
        assert_eq!(faas_trigger("schedule"), "timer");
        assert_eq!(faas_trigger("manual"), "other");
        assert_eq!(faas_trigger("airway"), "other");
    }

    /// `warehouse.upsert` and `ctx.tx` on a ClickHouse destination are refused
    /// by name before a statement is sent (`upsert_support::check`, the
    /// connector's `transaction::unsupported`), as `reply_json` prefixes them.
    /// The app chose the destination, and the refusal is the same on every
    /// call, so it is the app's condition — and the platform canary pins both
    /// messages on its ClickHouse destination every five minutes, which must
    /// not page.
    #[test]
    fn an_op_the_destinations_engine_cannot_do_is_a_bad_request() {
        for message in [
            "ctx.warehouse: warehouse.upsert is not supported on ClickHouse: it compiles to \
             `INSERT … ON CONFLICT … DO UPDATE`, which only Postgres and DuckDB parse. \
             Use warehouse.insert, or warehouse.exec with this warehouse's own upsert \
             statement.",
            "ctx.tx: could not open a transaction on 'canary_warehouse': ClickHouse does not \
             support multi-statement transactions — ctx.tx() requires a Postgres-backed \
             database (`type: postgres`). Use ctx.warehouse.{insert,exec,upsert} for \
             single-statement writes.",
        ] {
            let kind = classify_host_error(message);
            assert_eq!(kind, "bad_request", "{message}");
            assert!(!counts_toward_paging(kind), "{message}");
        }
    }

    // ── HOST_OPS is pinned to its two sources ──────────────────────────────
    //
    // `host_op_name` (this file) names the ops of the families with sub-ops;
    // `runtime::host_call_span` names the rest with one literal each. Both are
    // read from source here, so a name added to either without `HOST_OPS`, or
    // to `HOST_OPS` without a source, fails.
    //
    // The scanning helpers below are a smaller copy of
    // `tests/common/source_scan.rs`: a `src` unit test cannot reach an
    // integration binary's `common` module.

    /// The `{ … }` body of `fn <name>(` in `src`, braces counted outside
    /// string literals, `//` comment lines dropped. Enough for the two
    /// functions read here.
    fn fn_body(src: &str, name: &str) -> String {
        let at = src
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("`fn {name}` not found"));
        let code: String = src[at..]
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        let open = code.find('{').expect("a function body");
        let (mut depth, mut in_str, mut prev) = (0usize, false, '\0');
        for (i, c) in code[open..].char_indices() {
            match c {
                '"' if prev != '\\' => in_str = !in_str,
                '{' if !in_str => depth += 1,
                '}' if !in_str => {
                    depth -= 1;
                    if depth == 0 {
                        return code[open..open + i + 1].to_string();
                    }
                }
                _ => {}
            }
            prev = c;
        }
        panic!("unbalanced braces in `fn {name}`")
    }

    /// The contents of the string literal `s` opens with, if it opens with one.
    fn quoted(s: &str) -> Option<String> {
        s.strip_prefix('"')?
            .split_once('"')
            .map(|(inner, _)| inner.to_string())
    }

    /// Every `"…"` in `s`, in order (no escapes expected).
    fn string_literals(s: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = s;
        while let Some(at) = rest.find('"') {
            match quoted(&rest[at..]) {
                Some(inner) => {
                    rest = &rest[at + inner.len() + 2..];
                    out.push(inner);
                }
                None => break,
            }
        }
        out
    }

    /// Each arm of `host_op_name` as `(family, op, name)`; `op` is `None` for a
    /// family's `_` arm. The bare `_ => "other"` arm is not an op and is skipped.
    fn host_op_name_arms() -> Vec<(String, Option<String>, String)> {
        fn_body(include_str!("host_call_attrs.rs"), "host_op_name")
            .lines()
            .filter_map(|line| {
                let (lhs, rhs) = line.trim().split_once("=>")?;
                let name = quoted(rhs.trim())?;
                let (family, op) = lhs
                    .trim()
                    .trim_start_matches('(')
                    .trim_end_matches(')')
                    .split_once(',')?;
                Some((quoted(family.trim())?, quoted(op.trim()), name))
            })
            .collect()
    }

    /// The families `host_op_name` owns: those with a `(family, _)` arm.
    fn host_op_name_families() -> BTreeSet<String> {
        host_op_name_arms()
            .into_iter()
            .filter(|(_, op, _)| op.is_none())
            .map(|(family, _, _)| family)
            .collect()
    }

    /// The names `host_call_span` hands back itself: the literal after
    /// `HostCallKind::<kind>,` in an arm, and both in the query arm's
    /// `let op = if streaming { … } else { … };`.
    fn host_call_span_literals(body: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        for (at, marker) in body.match_indices("HostCallKind::") {
            let rest = &body[at + marker.len()..];
            let rest = rest.trim_start_matches(|c: char| c.is_alphanumeric());
            if let Some(name) = rest.strip_prefix(',').and_then(|r| quoted(r.trim_start())) {
                names.insert(name);
            }
        }
        let at = body
            .find("let op = ")
            .expect("the query arm's `let op = …;`");
        let end = body[at..].find(';').map_or(body.len(), |e| at + e);
        names.extend(string_literals(&body[at..end]));
        names
    }

    #[test]
    fn host_ops_is_every_name_host_op_name_returns_and_nothing_else() {
        let arms = host_op_name_arms();
        assert!(
            arms.len() > 20,
            "the arm scan found only {} arms",
            arms.len()
        );
        let families = host_op_name_families();
        let mut named = BTreeSet::new();
        for (family, op, name) in &arms {
            match op {
                Some(op) => {
                    assert_eq!(host_op_name(family, op), name, "arm ({family}, {op})");
                    assert!(
                        HOST_OPS.contains(&name.as_str()),
                        "`host_op_name` returns `{name}`, which HOST_OPS lacks"
                    );
                    named.insert(name.clone());
                }
                None => assert!(
                    name.ends_with("other"),
                    "the `_` arm of `{family}` names `{name}`, not `{family}.other`"
                ),
            }
        }
        for name in HOST_OPS {
            if let Some((family, _)) = name.split_once('.') {
                if families.contains(family) {
                    assert!(
                        named.contains(*name),
                        "HOST_OPS names `{name}`, which no `host_op_name` arm returns"
                    );
                }
            }
        }
    }

    #[test]
    fn host_ops_is_every_name_host_call_span_returns_and_nothing_else() {
        let body = fn_body(include_str!("runtime.rs"), "host_call_span");
        let literals = host_call_span_literals(&body);
        assert!(
            literals.len() >= 5,
            "the literal scan found only {literals:?}"
        );
        for name in &literals {
            assert!(
                HOST_OPS.contains(&name.as_str()),
                "`host_call_span` names `{name}`, which HOST_OPS lacks"
            );
        }
        let families = host_op_name_families();
        for name in HOST_OPS {
            match name.split_once('.') {
                Some((family, _)) if families.contains(family) => assert!(
                    body.contains(&format!("host_op_name(\"{family}\"")),
                    "`host_call_span` no longer names the `{family}` family through `host_op_name`"
                ),
                _ => assert!(
                    literals.contains(*name),
                    "HOST_OPS names `{name}`, which `host_call_span` never returns"
                ),
            }
        }
    }

    #[test]
    fn host_ops_has_no_duplicate() {
        let unique: BTreeSet<&str> = HOST_OPS.iter().copied().collect();
        assert_eq!(unique.len(), HOST_OPS.len());
    }
}
