//! What a caller's text can and cannot do to a query in `custom_apps`.
//!
//! Two mistakes were live here at once, and neither was visible to a test
//! that only looked for a doubled quote:
//!
//! - **a literal that could be ended early.** ClickHouse reads backslash
//!   escapes in a string literal, so `\'` after quote-doubling is an escaped
//!   quote followed by the terminator. The caller's own filters are now bound
//!   and never written; every other value is escaped for the backslash first.
//! - **a `SELECT` alias that shadowed the column a filter read.** ClickHouse
//!   resolves a name in `WHERE` to an alias before the column, so the window
//!   of `/logs` and the build filter of `/errors` failed on every read.
//!
//! Both were reproduced against ClickHouse 25.8, which no unit test here
//! reaches: these pin the query text that was verified there.

use super::*;

/// What a caller types to end a literal early. A doubled quote alone stops
/// only the last two: ClickHouse reads `\'` as an escaped quote, so a
/// backslash in front of a doubled quote turns its second half into the
/// terminator.
const BREAKOUTS: &[&str] = &[
    r"\",
    r"\' OR 1=1 -- ",
    r"x\' OR 1=1 -- ",
    r"\\' OR 1=1 -- ",
    r"\\",
    "' OR '1'='1",
    "it's",
];

/// Reads one single-quoted literal off the front of `sql` the way
/// ClickHouse's lexer does — a backslash takes the next character with it,
/// a doubled quote is a quote, a lone quote ends the literal — and returns
/// the value it holds and the SQL that follows it.
fn read_literal(sql: &str) -> Option<(String, &str)> {
    let body = sql.strip_prefix('\'')?;
    let mut chars = body.char_indices().peekable();
    let mut value = String::new();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => value.push(chars.next()?.1),
            '\'' if matches!(chars.peek(), Some((_, '\''))) => {
                chars.next();
                value.push('\'');
            }
            '\'' => return Some((value, &body[at + 1..])),
            c => value.push(c),
        }
    }
    None
}

#[test]
fn a_backslash_or_a_quote_cannot_end_a_literal_early() {
    for payload in BREAKOUTS {
        let sql = format!("'{}' AND tail", escape_sql_literal(payload));
        let (value, rest) = read_literal(&sql)
            .unwrap_or_else(|| panic!("{payload:?} left the literal open: {sql}"));
        assert_eq!(&value, payload, "the literal must hold the value: {sql}");
        assert_eq!(rest, " AND tail", "{payload:?} ended the literal: {sql}");
    }
}

/// `/errors?build_id=` reaches this query from a query string.
#[test]
fn a_build_id_cannot_break_out_of_the_client_error_query() {
    for payload in BREAKOUTS {
        let sql = client_errors_sql("o", "a", 24, 50, payload);
        let (_, after) = sql.split_once(" AND build_id = ").expect("a build clause");
        let (value, rest) = read_literal(after)
            .unwrap_or_else(|| panic!("{payload:?} left the literal open: {sql}"));
        assert_eq!(&value, payload, "{sql}");
        assert!(
            rest.starts_with(" GROUP BY stack_hash"),
            "{payload:?} reached the SQL after its literal: {sql}"
        );
    }
}

/// ClickHouse resolves a name in `WHERE` to a `SELECT` alias before the
/// column, so `argMax(build_id, ..) AS build_id` turned the build filter
/// into an aggregate inside `WHERE` and every filtered read failed
/// (`ILLEGAL_AGGREGATION`). The alias has its own name; the filter reads
/// the column.
#[test]
fn the_build_filter_reads_the_column_not_a_select_alias() {
    let sql = client_errors_sql("o", "a", 24, 50, "b1");
    assert!(
        sql.contains("argMax(build_id, timestamp) AS latest_build_id,"),
        "{sql}"
    );
    assert!(!sql.contains(" AS build_id"), "{sql}");
    assert!(sql.contains(" AND build_id = 'b1' GROUP BY"), "{sql}");
}

/// `/logs?invocation_id=&request_id=` arrive from a query string too, and
/// are bound: the query text carries a placeholder and the value travels
/// beside it, so there is no literal for either to break out of.
#[test]
fn function_log_filters_are_bound_not_written_into_the_query() {
    assert_eq!(
        bound_equals("invocation_id"),
        " AND invocation_id = {invocation_id:String}"
    );
    assert_eq!(
        bound_equals("request_id"),
        " AND request_id = {request_id:String}"
    );
    for payload in BREAKOUTS {
        assert_eq!(
            function_log_params(payload, payload),
            vec![("invocation_id", *payload), ("request_id", *payload)],
            "a bound value is passed as typed, unescaped"
        );
    }
    assert_eq!(function_log_params("", "r"), vec![("request_id", "r")]);
    assert_eq!(
        function_log_params("", ""),
        vec![],
        "an empty filter has no placeholder, so it binds nothing"
    );
}

/// The name reaches the predicate from a query string. The route parses
/// it first, but this crate does not trust that: whatever it is, it stays
/// one literal. Doubling the quote is not enough for that — ClickHouse
/// reads `\'` as an escaped quote — so the backslash is doubled too.
#[test]
fn function_logs_escape_the_environment_name() {
    let sql = function_logs_sql("o", "a", 24, 50, "", "", "x' OR '1'='1");
    assert!(
        sql.contains("AND environment = 'x'' OR ''1''=''1' "),
        "{sql}"
    );
    for payload in BREAKOUTS {
        let sql = function_logs_sql("o", "a", 24, 50, "", "", payload);
        let (_, after) = sql.split_once("AND environment = ").expect("the clause");
        let (value, rest) = read_literal(after)
            .unwrap_or_else(|| panic!("{payload:?} left the literal open: {sql}"));
        assert_eq!(&value, payload, "{sql}");
        assert!(
            rest.starts_with(" AND timestamp >= now() - INTERVAL 24 HOUR ORDER BY"),
            "{payload:?} reached the SQL after its literal: {sql}"
        );
    }
}

/// `invocation_id` and `request_id` are the caller's to type. Neither is
/// written into the query in any form: the text carries a placeholder,
/// the same text whatever was typed, and the value is bound beside it.
#[test]
fn function_logs_never_write_a_filter_value_into_the_query() {
    let honest = function_logs_sql("o", "a", 24, 50, "inv", "req", "");
    assert!(
        honest.contains(
            " HOUR AND invocation_id = {invocation_id:String} \
             AND request_id = {request_id:String} ORDER BY"
        ),
        "{honest}"
    );
    for payload in BREAKOUTS {
        assert_eq!(
            function_logs_sql("o", "a", 24, 50, payload, payload, ""),
            honest,
            "{payload:?} changed the query text"
        );
    }
    // A filter left empty has no clause, so nothing is bound for it.
    let unfiltered = function_logs_sql("o", "a", 24, 50, "", "", "");
    assert!(!unfiltered.contains("invocation_id ="), "{unfiltered}");
    assert!(!unfiltered.contains("request_id ="), "{unfiltered}");
}

/// ClickHouse resolves a name in `WHERE` to a `SELECT` alias before the
/// column: aliasing the formatted timestamp `AS timestamp` made the
/// window compare a String with a DateTime and every read fail
/// (`NO_COMMON_TYPE`). The alias has its own name, as
/// `execution_list_data_sql`'s does, in every environment.
#[test]
fn function_logs_window_reads_the_column_not_the_formatted_alias() {
    for environment in ["", "staging"] {
        let sql = function_logs_sql("o", "a", 24, 50, "", "", environment);
        assert!(sql.contains(" AS timestamp_iso, "), "{sql}");
        assert!(!sql.contains(" AS timestamp, "), "{sql}");
        assert!(
            sql.contains("AND timestamp >= now() - INTERVAL 24 HOUR"),
            "{sql}"
        );
    }
}
