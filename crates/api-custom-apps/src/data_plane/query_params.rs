//! `params` on `POST /api/projects/{project_id}/query`: the values of a SQL
//! template's placeholders, written into it here rather than in the browser.
//!
//! A template names a param two ways, and they are the whole grammar:
//!
//! - `{{ params.X | sqlquote }}` — a string as a `'…'` literal, a number or
//!   boolean as itself, a missing or `null` param as `NULL`;
//! - `{{ params.X }}` — the value as it is, unquoted. The caller answers for it.
//!
//! The SDK's `useQuery` used to do this itself, doubling `'` and nothing else,
//! and send the finished SQL. That is how DuckDB and Postgres read a literal.
//! ClickHouse, MySQL, Snowflake, Redshift and Domo also read a backslash as an
//! escape, and BigQuery reads a backslash and does not read `''` as a quote, so
//! a value holding a backslash broke the query there or changed what it asked.
//! The browser cannot know which engine a project's database is. This side
//! does, so the value is written by that engine's rule.
//!
//! **This is a substitution, not a template render.** `sql` is request text:
//! handing it to a Jinja environment would evaluate whatever expression the
//! caller put between `{{` and `}}`. One regex pass replaces the two
//! placeholder forms and leaves every other character alone — including the
//! output of an earlier replacement, so a value spelling `{{ params.Y }}` is
//! written as those characters.
//!
//! It is not a privilege boundary either. The endpoint runs whatever read-only
//! SQL a member sends; this is about a value a user typed reaching the
//! warehouse as that value.

use std::sync::LazyLock;

use agentic_connector::StringLiteral;
use regex::{Captures, Regex};
use serde_json::{Map, Value};

/// The same two forms the SDK's `paramsToSend` looks for.
static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{\{\s*params\.([a-zA-Z0-9_]+)(\s*\|\s*sqlquote)?\s*\}\}")
        .expect("the placeholder pattern is a valid regex")
});

/// `sql` with each placeholder replaced by its param's value, a string under
/// `| sqlquote` written by `literal` — the rule of the engine `sql` is sent to.
///
/// The caller resolves `sql`'s database to a concrete engine before calling
/// this — [`query.rs`](super::query) answers a database it cannot resolve
/// as its own bad-request, and every resolved database type has a rule
/// (`agentic_wiring::string_literal::string_literal_of` is an exhaustive
/// match), so there is no "unnamed engine" case left for this function to
/// refuse: every value `| sqlquote`s by a rule that quotes it. The error
/// this returns is the caller's own to fix and is safe to return to it: it
/// names the param, never its value.
pub(super) fn bind_params(
    sql: &str,
    params: &Map<String, Value>,
    literal: StringLiteral,
) -> Result<String, String> {
    let mut refused: Option<String> = None;
    let bound = PLACEHOLDER.replace_all(sql, |found: &Captures<'_>| {
        let name = &found[1];
        let quoted = found.get(2).is_some();
        written(params.get(name), quoted, literal).unwrap_or_else(|why| {
            refused.get_or_insert(format!("param `{name}`: {why}"));
            String::new()
        })
    });
    match refused {
        Some(why) => Err(why),
        None => Ok(bound.into_owned()),
    }
}

/// One param's value as SQL text.
fn written(value: Option<&Value>, quoted: bool, literal: StringLiteral) -> Result<String, String> {
    match value {
        None | Some(Value::Null) => Ok("NULL".to_string()),
        Some(Value::Bool(flag)) => Ok(flag.to_string()),
        Some(Value::Number(number)) => Ok(number.to_string()),
        Some(Value::String(text)) if !quoted => Ok(text.clone()),
        Some(Value::String(text)) => Ok(literal.quote(text)),
        Some(Value::Array(_) | Value::Object(_)) => {
            Err("must be a string, a number, a boolean or null".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const RULES: [StringLiteral; 3] = [
        StringLiteral::Standard,
        StringLiteral::Backslash,
        StringLiteral::BackslashOnly,
    ];

    const SQL: &str = "SELECT * FROM t WHERE name = {{ params.name | sqlquote }} AND tail";

    fn params(value: Value) -> Map<String, Value> {
        value.as_object().expect("an object").clone()
    }

    fn bound(literal: StringLiteral, name: &str) -> Result<String, String> {
        bind_params(SQL, &params(json!({ "name": name })), literal)
    }

    /// The value the old client-side quoting broke out with on every engine
    /// that reads a backslash, written by each engine's own rule.
    #[test]
    fn a_string_is_written_by_the_rule_of_the_engine_that_reads_it() {
        let breakout = "x\\' OR 1=1 -- ";
        for (rule, want) in [
            (StringLiteral::Standard, "'x\\'' OR 1=1 -- '"),
            (StringLiteral::Backslash, "'x\\\\'' OR 1=1 -- '"),
            (StringLiteral::BackslashOnly, "'x\\\\\\' OR 1=1 -- '"),
        ] {
            assert_eq!(
                bound(rule, breakout).unwrap(),
                format!("SELECT * FROM t WHERE name = {want} AND tail"),
                "{rule:?}"
            );
        }
        // A trailing backslash no longer swallows the closing quote.
        assert_eq!(
            bound(StringLiteral::Backslash, "C:\\").unwrap(),
            "SELECT * FROM t WHERE name = 'C:\\\\' AND tail"
        );
    }

    #[test]
    fn a_plain_value_is_the_same_bytes_on_every_engine() {
        let want = "SELECT * FROM t WHERE name = 'west' AND tail";
        for rule in RULES {
            assert_eq!(bound(rule, "west").unwrap(), want, "{rule:?}");
        }
    }

    #[test]
    fn a_number_a_boolean_and_null_are_written_as_themselves() {
        let sql = "SELECT {{ params.n | sqlquote }}, {{ params.f|sqlquote }}, \
                   {{params.b | sqlquote}}, {{ params.z | sqlquote }}, {{ params.missing | sqlquote }}";
        let values = params(json!({ "n": 42, "f": 1.5, "b": true, "z": null }));
        assert_eq!(
            bind_params(sql, &values, StringLiteral::Standard).unwrap(),
            "SELECT 42, 1.5, true, NULL, NULL"
        );
    }

    /// `{{ params.X }}` with no filter is the caller's own text, unquoted —
    /// under every rule, since no literal is written.
    #[test]
    fn a_param_with_no_filter_is_written_unquoted() {
        let sql = "SELECT * FROM {{ params.table }} LIMIT {{params.n}} -- {{ params.missing }}";
        let values = params(json!({ "table": "orders", "n": 5 }));
        for literal in RULES {
            assert_eq!(
                bind_params(sql, &values, literal).unwrap(),
                "SELECT * FROM orders LIMIT 5 -- NULL",
                "{literal:?}"
            );
        }
    }

    /// One pass: what a value spells is never read as a placeholder, quoted
    /// or not.
    #[test]
    fn a_value_that_spells_a_placeholder_is_not_substituted_again() {
        let sql = "SELECT {{ params.a | sqlquote }}, {{ params.b }}";
        let values = params(json!({
            "a": "{{ params.secret | sqlquote }}",
            "b": "{{ params.secret }}",
            "secret": "leaked",
        }));
        assert_eq!(
            bind_params(sql, &values, StringLiteral::Standard).unwrap(),
            "SELECT '{{ params.secret | sqlquote }}', {{ params.secret }}"
        );
    }

    /// Only the two placeholder forms are touched. Anything else between
    /// braces is the caller's SQL and stays as it is — nothing is evaluated.
    #[test]
    fn nothing_but_a_placeholder_is_replaced() {
        for sql in [
            "SELECT 1",
            "SELECT '{{ 7 * 7 }}', '{{ now() }}', '{% if x %}'",
            "SELECT '{{ controls.store }}', '{{ params }}', '{{ params.a | upper }}'",
        ] {
            let values = params(json!({ "a": "v" }));
            assert_eq!(
                bind_params(sql, &values, StringLiteral::Standard).unwrap(),
                sql
            );
        }
    }

    #[test]
    fn a_param_that_is_not_a_scalar_is_refused() {
        for value in [json!(["a"]), json!({ "a": 1 })] {
            let refused = bind_params(
                SQL,
                &params(json!({ "name": value })),
                StringLiteral::Standard,
            )
            .unwrap_err();
            assert_eq!(
                refused,
                "param `name`: must be a string, a number, a boolean or null"
            );
        }
    }
}
