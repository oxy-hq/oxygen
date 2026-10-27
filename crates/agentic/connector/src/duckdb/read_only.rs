//! Which SQL a connector may run on a clone of a shared directory database.
//!
//! A clone shares its catalog with every other connector checked out of the
//! same [`super::dir_cache`] entry, so a statement that writes — `CREATE
//! TABLE`, `DROP VIEW`, an `INSERT`, a global `SET` — would reach every run
//! reading that directory, and outlive the run that issued it. Before such a
//! statement the connector moves onto a private database (see
//! [`super::session::Sessions::turn_for`]).
//!
//! The classifier is deliberately conservative. Calling a read a write costs
//! one private rebuild, which is what every connector paid before the cache
//! existed. Calling a write a read leaks it into other runs. So a statement is
//! read-only only when the scanner accounted for every character of it, its
//! first word opens a query, and no word in it can start or carry a write.
//! Text the scanner cannot place — an unclosed quote, an `E'…'` escape string,
//! a `$` that is not a dollar quote — makes the statement a write.
//!
//! A `SELECT` can also change the database without writing a row: DuckDB's
//! instance-level table functions (`enable_logging()` turns logging on for
//! every connection to the database, and its log store then grows with each
//! run) and the ones that run SQL built at run time (`query(…)`, which can
//! assemble any of those names from pieces). DuckDB's parser types all of
//! them as a `SELECT`, so the parse check cannot see them: a statement that
//! calls one is a write here ([`INSTANCE_FUNCTIONS`]).
//!
//! **This is the fast router, not the guarantee.** A hand-written lexer can
//! disagree with DuckDB's grammar, and each such gap has been a way to hide a
//! write. So a script it calls read-only stays on the shared catalog only if
//! DuckDB's own parser agrees ([`super::parse_check`]): both must say read.

use crate::connector::{dollar_tag_len, follows_identifier_char, split_sql_statements};

/// First words of a statement that only reads.
const READ_FIRST: &[&str] = &[
    "SELECT",
    "WITH",
    "FROM", // DuckDB's `FROM tbl` shorthand
    "VALUES",
    "TABLE",
    "SHOW",
    "DESCRIBE",
    "DESC",
    "SUMMARIZE",
    "PIVOT",
    "UNPIVOT",
];

/// Words that make an otherwise read-shaped statement a write: a
/// data-modifying CTE (`WITH … INSERT`), `EXPLAIN ANALYZE` of a write (it
/// executes the statement), `COPY … TO`.
const WRITES: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "MERGE", "TRUNCATE", "CREATE", "DROP", "ALTER", "COPY", "INTO",
    "ATTACH", "DETACH",
];

/// Table functions that act on the database instance rather than read from
/// it, or run SQL built at run time. Each of DuckDB's table functions is
/// classified in `every_duckdb_table_function_is_classified`, which fails
/// when a DuckDB upgrade brings one nobody has looked at.
const INSTANCE_FUNCTIONS: &[&str] = &[
    "ENABLE_LOGGING",
    "DISABLE_LOGGING",
    "ENABLE_PROFILING",
    "DISABLE_PROFILING",
    "TRUNCATE_DUCKDB_LOGS",
    "CHECKPOINT",
    "FORCE_CHECKPOINT",
    "QUERY",
    "JSON_EXECUTE_SERIALIZED_SQL",
];

/// `true` when every statement in `sql` only reads. An empty script reads
/// nothing, so it is read-only.
pub(super) fn is_read_only(sql: &str) -> bool {
    split_sql_statements(sql)
        .iter()
        .all(|stmt| statement_is_read_only(stmt))
}

fn statement_is_read_only(stmt: &str) -> bool {
    let Some(Scanned { words, calls }) = scan(stmt) else {
        return false;
    };
    if calls
        .iter()
        .any(|f| INSTANCE_FUNCTIONS.contains(&f.as_str()))
    {
        return false;
    }
    let mut rest = words.iter().map(String::as_str);
    let first = match rest.next() {
        Some("EXPLAIN") => {
            // `EXPLAIN [ANALYZE] <stmt>`: ANALYZE executes `<stmt>`, so it is
            // judged like `<stmt>` itself.
            let mut next = rest.next();
            if matches!(next, Some("ANALYZE" | "ANALYSE")) {
                next = rest.next();
            }
            next
        }
        other => other,
    };
    let Some(first) = first else {
        return true;
    };
    READ_FIRST.contains(&first) && !words.iter().any(|w| WRITES.contains(&w.as_str()))
}

/// What [`scan`] reads out of one statement.
struct Scanned {
    /// The unquoted words, uppercased. A word is `[A-Za-z_][A-Za-z0-9_]*`;
    /// numbers and punctuation are dropped.
    words: Vec<String>,
    /// Every name called as a function, uppercased: a word or a quoted
    /// identifier (`"query"`) whose next token, past spaces and comments, is
    /// `(`. A schema-qualified call (`system.enable_logging()`) is its last
    /// name.
    calls: Vec<String>,
}

/// The words and calls of `stmt`, skipping string literals, dollar-quoted
/// strings and comments; a quoted identifier is a call when one follows it,
/// and never a word.
///
/// `None` when the scanner cannot account for every character — a quote,
/// dollar quote or comment still open at the end, an `E'…'` escape string, a
/// `$` that is not a dollar quote (one inside a name, a parameter). Its caller reads that as a
/// write: a run's statement only stays on the shared catalog when every word
/// in it was seen. Dollar quotes are matched as `split_sql_statements` matches
/// them (`dollar_tag_len`), so the two scanners agree on where text is.
fn scan(stmt: &str) -> Option<Scanned> {
    let chars: Vec<char> = stmt.chars().collect();
    let mut words = Vec::new();
    let mut calls = Vec::new();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\'' => i = skip_quoted(&chars, i)?,
            '"' | '`' => {
                let end = skip_quoted(&chars, i)?;
                if opens_call(&chars, end) {
                    let name: String = chars[i + 1..end - 1].iter().collect();
                    let quote = c.to_string();
                    calls.push(name.replace(&quote.repeat(2), &quote).to_ascii_uppercase());
                }
                i = end;
            }
            // A `$` that continues a name or number belongs to it (`x$y$` is
            // one identifier to DuckDB). Not modelled: unreadable.
            '$' if follows_identifier_char(&chars, i) => return None,
            '$' => i = skip_dollar_quoted(&chars, i)?,
            '-' if chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                if i >= chars.len() {
                    return None;
                }
                i += 2;
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
                let word: String = chars[start..i].iter().collect();
                if word.eq_ignore_ascii_case("e") && chars.get(i) == Some(&'\'') {
                    // `E'…'` reads backslash escapes.
                    return None;
                }
                if opens_call(&chars, i) {
                    calls.push(word.to_ascii_uppercase());
                }
                words.push(word.to_ascii_uppercase());
            }
            c if c.is_ascii_digit() => {
                // A number, or the tail of one (`1e10`): not a word.
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    Some(Scanned { words, calls })
}

/// Whether the next token at or after `i`, past spaces and comments, is `(`.
/// A comment that never closes reads as one, so its statement is judged as
/// a call rather than waved through.
fn opens_call(chars: &[char], mut i: usize) -> bool {
    while i < chars.len() {
        match chars[i] {
            c if c.is_whitespace() => i += 1,
            '-' if chars.get(i + 1) == Some(&'-') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if chars.get(i + 1) == Some(&'*') => {
                i += 2;
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    i += 1;
                }
                if i >= chars.len() {
                    return true;
                }
                i += 2;
            }
            c => return c == '(',
        }
    }
    false
}

/// Index just past the quoted run that opens at `start`; a doubled quote is
/// an escape. `None` when it never closes.
fn skip_quoted(chars: &[char], start: usize) -> Option<usize> {
    let quote = chars[start];
    let mut i = start + 1;
    while i < chars.len() {
        if chars[i] == quote {
            if chars.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return Some(i + 1);
        }
        i += 1;
    }
    None
}

/// Index just past the `$tag$…$tag$` string that opens at `start`. `None` for
/// a `$` that opens no dollar quote (a parameter, a `$` in a name), or one
/// that never closes.
fn skip_dollar_quoted(chars: &[char], start: usize) -> Option<usize> {
    let tag_len = dollar_tag_len(&chars[start..])?;
    let tag = &chars[start..start + tag_len];
    let mut i = start + tag_len;
    while i < chars.len() {
        if chars[i..].starts_with(tag) {
            return Some(i + tag_len);
        }
        i += 1;
    }
    None
}

#[cfg(test)]
#[path = "read_only_tests.rs"]
mod tests;
