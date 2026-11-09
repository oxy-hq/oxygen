//! Credential scrubbing for text that leaves a connector as an error.
//!
//! A connector error carries the failing statement and the driver's message,
//! and both can quote a credential: DuckDB repeats a whole `ATTACH` (libpq
//! `password=` and all) in a parser error, a URL can carry `user:pass@`, a
//! `CREATE SECRET` names its key. That text is shown on Workspace Health,
//! returned to chat and tool calls, and logged. Callers that know their secret
//! values scrub them by value as well (`DuckDbRawConfig::redact`,
//! `DuckLakeAttach::redact`); this module
//! is the backstop that needs no list, matching the *shapes* a credential
//! takes instead.
//!
//! It errs toward hiding: `WHERE token = 'abc'` in an echoed query loses its
//! literal too. That costs a little error detail and never a credential.

/// Keys whose value is a credential, matched case-insensitively as a whole
/// word (`client_secret` is its own key, not `secret` inside it).
const SECRET_KEYS: &[&str] = &[
    "aws_secret_access_key",
    "secret_access_key",
    "private_key_passphrase",
    "motherduck_token",
    "refresh_token",
    "session_token",
    "access_token",
    "client_secret",
    "account_key",
    "accountkey",
    "private_key",
    "sslpassword",
    "sas_token",
    "password",
    "api_key",
    "apikey",
    "passwd",
    "secret",
    "token",
    "pwd",
];

/// What a redacted value is replaced with.
pub const REDACTED: &str = "***";

/// `text` with every credential-shaped value replaced by [`REDACTED`]:
///
/// - `key=value` and `key = 'value'` for a [`SECRET_KEYS`] key — libpq
///   keyword/value strings, URL query parameters;
/// - `KEY 'value'` — DuckDB `CREATE SECRET (… PASSWORD '…', SECRET '…')`;
/// - the password in a URL's `scheme://user:password@host`.
///
/// Host, user, database and every other part of the text are left as they are.
pub fn redact_credentials(text: &str) -> String {
    redact_url_userinfo(&redact_keyed_values(text))
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn skip_blanks(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    i
}

/// The length of the secret key starting at `i`, if a whole-word one does.
fn key_at(bytes: &[u8], i: usize) -> Option<usize> {
    if i > 0 && is_ident_byte(bytes[i - 1]) {
        return None;
    }
    SECRET_KEYS.iter().find_map(|key| {
        let end = i + key.len();
        let matches = end <= bytes.len()
            && bytes[i..end].eq_ignore_ascii_case(key.as_bytes())
            && !bytes.get(end).is_some_and(|b| is_ident_byte(*b));
        matches.then_some(key.len())
    })
}

/// The end (exclusive) of a quoted value whose opening quote is at `start`:
/// the index of the closing quote, or the end of the text when unterminated.
/// A backslash escapes the next byte and a doubled quote is a literal one.
fn quoted_end(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b if b == quote && bytes.get(i + 1) == Some(&quote) => i += 2,
            b if b == quote => return i,
            _ => i += 1,
        }
    }
    bytes.len()
}

/// The value range of a quoted value whose opening quote is at `start`.
///
/// A doubled opening quote is the delimiter of a libpq-quoted value inside a
/// SQL literal (`''p w''`), and the value runs to the matching doubled quote —
/// read as an empty literal, the password after it would leak. It is an empty
/// literal only when a delimiter follows and no closing `''` comes later on
/// the line (`token = '' AND …`, `PASSWORD '')`).
fn quoted_range(bytes: &[u8], start: usize) -> (usize, usize) {
    let quote = bytes[start];
    if bytes.get(start + 1) != Some(&quote) {
        return (start + 1, quoted_end(bytes, start));
    }
    let open = start + 2;
    let (stop, closed) = doubled_close(bytes, open, quote);
    let delimiter_follows = bytes
        .get(open)
        .is_none_or(|b| b.is_ascii_whitespace() || matches!(b, b')' | b',' | b';'));
    if !closed && delimiter_follows {
        return (start + 1, start + 1);
    }
    (open, stop)
}

/// Where a value opened by a doubled quote at `open - 2` stops, and whether a
/// closing doubled quote stopped it (else the end of the line or text). A
/// backslash escapes the next byte: libpq writes a quote in a value as `\'`,
/// which a SQL literal doubles to `\''`.
fn doubled_close(bytes: &[u8], open: usize, quote: u8) -> (usize, bool) {
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i += 2,
            b'\n' => return (i, false),
            b if b == quote && bytes.get(i + 1) == Some(&quote) => return (i, true),
            _ => i += 1,
        }
    }
    (bytes.len(), false)
}

/// The end (exclusive) of a bare value: the first whitespace or quote. Not
/// `,` `)` `;` `&` — a password may hold any of them, and stopping there would
/// leak the rest of it. A URL query's later parameters are hidden with it.
fn bare_end(bytes: &[u8], start: usize) -> usize {
    let mut i = start;
    while i < bytes.len() && !bytes[i].is_ascii_whitespace() && !matches!(bytes[i], b'\'' | b'"') {
        i += 1;
    }
    i
}

/// The byte range of the value that follows a key ending at `key_end`, if the
/// key is followed by a value at all.
fn value_range(bytes: &[u8], key_end: usize) -> Option<(usize, usize)> {
    let after_key = skip_blanks(bytes, key_end);
    if bytes.get(after_key) == Some(&b'=') {
        let start = skip_blanks(bytes, after_key + 1);
        if matches!(bytes.get(start), Some(b'\'' | b'"')) {
            return Some(quoted_range(bytes, start));
        }
        let end = bare_end(bytes, start);
        return (end > start).then_some((start, end));
    }
    // `PASSWORD 'x'`: a blank, then a quoted literal. A bare word after a
    // blank is prose ("password authentication failed"), not a value.
    if after_key > key_end && matches!(bytes.get(after_key), Some(b'\'' | b'"')) {
        return Some(quoted_range(bytes, after_key));
    }
    None
}

fn redact_keyed_values(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    while i < bytes.len() {
        let Some(len) = key_at(bytes, i) else {
            i += 1;
            continue;
        };
        match value_range(bytes, i + len) {
            Some((start, end)) if end > start => {
                out.push_str(&text[copied..start]);
                out.push_str(REDACTED);
                copied = end;
                i = end.max(i + len);
            }
            _ => i += len,
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Every `scheme://user:password@host` with the password replaced. Errs
/// toward hiding: an `@` later in the path or query extends what is hidden.
fn redact_url_userinfo(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some(found) = text[search..].find("://") {
        let authority = search + found + 3;
        let mut end = authority;
        // Only whitespace or a quote ends the scan: a raw (not percent-encoded)
        // password may hold `/` `?` `#` `)`, so the userinfo is everything up to
        // the last `@` before that point.
        while end < bytes.len()
            && !bytes[end].is_ascii_whitespace()
            && !matches!(bytes[end], b'\'' | b'"')
        {
            end += 1;
        }
        if let Some(at) = text[authority..end].rfind('@') {
            let userinfo = &text[authority..authority + at];
            if let Some(colon) = userinfo.find(':') {
                let pass_start = authority + colon + 1;
                let pass_end = authority + at;
                if pass_end > pass_start {
                    out.push_str(&text[copied..pass_start]);
                    out.push_str(REDACTED);
                    copied = pass_end;
                }
            }
        }
        search = end.max(authority);
    }
    out.push_str(&text[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PW: &str = "synthetic-Pa55word";

    fn assert_hidden(text: &str) -> String {
        let out = redact_credentials(text);
        assert!(!out.contains("Pa55"), "{out}");
        assert!(out.contains(REDACTED), "{out}");
        out
    }

    #[test]
    fn a_libpq_password_is_hidden_and_the_rest_kept() {
        let out = assert_hidden(&format!(
            "ATTACH 'ducklake:postgres:user=ducklake password={PW} host=db.internal \
             dbname=ducklake_catalog sslmode=require' AS ducklake (DATA_PATH 's3://b/')"
        ));
        assert!(out.contains("user=ducklake"), "{out}");
        assert!(out.contains("host=db.internal"), "{out}");
        assert!(out.contains("dbname=ducklake_catalog"), "{out}");
        assert!(out.contains("sslmode=require"), "{out}");
        assert!(out.contains("s3://b/"), "{out}");
    }

    #[test]
    fn a_url_encoded_or_quoted_password_is_hidden() {
        assert_hidden("password=synthetic%2FPa55%40word host=x");
        assert_hidden("PASSWORD='synthetic Pa55 \\' word' host=x");
        assert_hidden("Password = \"synthetic Pa55\"");
    }

    #[test]
    fn a_duckdb_secret_literal_is_hidden() {
        assert_hidden(&format!(
            "CREATE OR REPLACE SECRET (TYPE postgres, PASSWORD '{PW}')"
        ));
        assert_hidden("CREATE SECRET (TYPE s3, KEY_ID 'AKIA', SECRET 'Pa55''s', REGION 'x')");
        assert_hidden("CREATE SECRET (TYPE azure, ACCOUNT_KEY 'Pa55')");
    }

    #[test]
    fn a_url_password_is_hidden_in_userinfo_and_query() {
        let out = assert_hidden(&format!("mysql://reader:{PW}@db.internal:3306/sales"));
        assert!(
            out.contains("mysql://reader:***@db.internal:3306/sales"),
            "{out}"
        );
        assert_hidden("postgres://u@h/db?sslmode=require&password=Pa55&x=1");
        assert_hidden("md:my_db?motherduck_token=Pa55.jwt");
    }

    #[test]
    fn prose_and_ordinary_sql_are_left_alone() {
        for text in [
            "password authentication failed for user \"ducklake\"",
            "Access denied for user 'r'@'h' (using password: YES)",
            "SELECT client_id, my_token_count FROM t WHERE x = 1",
            "CREATE SECRET (TYPE s3, PROVIDER credential_chain)",
            "https://example.com/path?page=2",
            "user@example.com wrote to s3://bucket/key",
            "naïve résumé — ünïcode",
        ] {
            assert_eq!(redact_credentials(text), text);
        }
    }

    /// A bare value ends only at whitespace or a quote: a password may hold
    /// `,` `)` `;` `&`, and stopping at one would leak the rest of it.
    #[test]
    fn a_bare_password_holding_punctuation_is_hidden_whole() {
        let out = assert_hidden("password=Xy7,k9)Q;z&w host=h");
        assert!(!out.contains("k9") && !out.contains("z&w"), "{out}");
        assert!(out.ends_with(" host=h"), "{out}");
    }

    /// A libpq-quoted value inside a SQL literal arrives as `''p w''`: the
    /// doubled quote opens the value, and the value runs to the matching `''`.
    #[test]
    fn a_doubled_quote_opened_password_is_hidden_whole() {
        let out = assert_hidden("ATTACH 'postgres:password=''p w Pa55'' host=h' AS x");
        assert!(!out.contains("p w"), "{out}");
        assert!(out.contains("host=h"), "{out}");
    }

    /// A quote inside a libpq-quoted value is `\'`, doubled again inside a
    /// SQL literal (`''p\''w''`): the escaped quote does not close the value.
    #[test]
    fn a_doubled_quote_opened_password_with_an_escaped_quote_is_hidden_whole() {
        let out = assert_hidden("password=''p\\''w Pa55'' host=h");
        assert!(!out.contains("w Pa") && !out.contains("Pa55"), "{out}");
        assert!(out.ends_with(" host=h"), "{out}");
    }

    /// `''` followed by a delimiter, with no closing `''` after it, is an
    /// empty literal: nothing to hide, and the rest of the text stays.
    #[test]
    fn an_empty_literal_after_a_secret_key_hides_nothing_after_it() {
        for text in [
            "SELECT * FROM t WHERE token = '' AND x = 1",
            "CREATE SECRET (TYPE postgres, PASSWORD '')",
        ] {
            assert_eq!(redact_credentials(text), text);
        }
    }

    /// A raw (not percent-encoded) URL password may hold `)` `#` `/` `?`.
    #[test]
    fn a_raw_url_password_holding_delimiters_is_hidden() {
        for (text, want) in [
            ("postgres://u:ab)cd@h/db", "postgres://u:***@h/db"),
            ("postgres://u:p#w@h/db", "postgres://u:***@h/db"),
            (
                "postgres://reader:p#w/x?y@db.internal/db x",
                "postgres://reader:***@db.internal/db x",
            ),
        ] {
            assert_eq!(redact_credentials(text), want);
        }
    }

    #[test]
    fn every_occurrence_is_hidden() {
        let out = assert_hidden("password=Pa55a x password=Pa55b SQL: password=Pa55c");
        assert_eq!(out.matches(REDACTED).count(), 3, "{out}");
    }
}
