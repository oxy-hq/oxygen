//! What a connection shared between requesters may run.
//!
//! The analytics pool hands one Airhouse connection to every requester of a
//! workspace at a role (`oxy-app`'s `project_ctx::airhouse_pool_key`), and
//! Airhouse backs each connection with one server-side DuckDB session. A
//! statement whose effect outlives itself on that session would therefore
//! reach the next requester: a `SET`, a temp table, view or secret, a table in
//! the session's in-memory catalog, a `USE`, an `ATTACH`, a `PRAGMA`, or a
//! transaction left open by `BEGIN`. A connector built with
//! [`super::AirhouseConnector::shared_session`] refuses all of that before
//! anything is sent.
//!
//! **An allowlist, failing closed.** A statement passes only when its leading
//! keyword is a read ([`READ_KEYWORDS`]) or a write to a persistent object
//! ([`WRITE_KEYWORDS`]); anything else is refused, so a session-changing
//! statement nobody listed is refused too. A write is also refused when it
//! creates or drops a temporary or secret object (a session word anywhere
//! among `CREATE`'s or `DROP`'s modifiers) or names the session-scoped `temp` or `memory`
//! catalog. A statement that runs another (`EXPLAIN ANALYZE`, `SUMMARIZE
//! <query>`) is judged by the statement it runs.
//!
//! **Its own lexer.** The batch is split here rather than with the shared
//! splitter, following DuckDB's quoting and identifier rules: `'…'` with `''`,
//! `E'…'` with backslash escapes, `"…"`, `$tag$…$tag$` (a tag starts with a
//! letter or `_`), `--` and `/* … */` comments, and identifiers that may hold
//! `$` after their first character, so `a$$` is a name and not the start of a
//! dollar quote. Where DuckDB's reading is in doubt (a `/*` inside a block
//! comment, which Postgres-family lexers nest; an unterminated quote or
//! comment) the batch is refused rather than guessed at. It reads keywords
//! rather than parsing, because the analytics path sends DuckDB SQL that
//! sqlparser does not always accept.

use agentic_connector::ConnectorError;

/// Leading keywords of statements that only read.
const READ_KEYWORDS: &[&str] = &[
    "SELECT", "WITH", "FROM", "TABLE", "VALUES", "SHOW", "PIVOT", "UNPIVOT",
];

/// Leading keywords of statements that run the statement, or read the
/// relation, after them. Judged by what follows ([`wrapped_reason`]).
const WRAPPING_KEYWORDS: &[&str] = &["EXPLAIN", "SUMMARIZE", "DESCRIBE", "DESC"];

/// Words `EXPLAIN` may take before the statement it explains.
const EXPLAIN_OPTIONS: &[&str] = &["ANALYZE", "ANALYSE", "VERBOSE"];

/// Leading keywords of statements that write to persistent objects. Each is
/// checked further by [`write_reaches_session`].
const WRITE_KEYWORDS: &[&str] = &[
    "INSERT", "UPDATE", "DELETE", "MERGE", "CREATE", "DROP", "ALTER", "TRUNCATE", "COMMENT",
];

/// Among `CREATE`'s or `DROP`'s modifiers, the words that name a
/// session-scoped object: a temp table, view, macro or sequence, or a secret
/// (temporary unless `PERSISTENT`, and never something a shared read
/// credential should create or remove: dropping one the session relies on
/// breaks every later requester at the key).
const SESSION_CREATE_WORDS: &[&str] = &["TEMP", "TEMPORARY", "SECRET", "PERSISTENT"];

/// The object-type keyword that ends `CREATE`'s or `DROP`'s modifiers.
const CREATE_OBJECT_TYPES: &[&str] = &[
    "TABLE", "VIEW", "MACRO", "FUNCTION", "SEQUENCE", "TYPE", "SCHEMA", "INDEX", "DATABASE",
];

/// How many wrapping keywords (`EXPLAIN EXPLAIN …`) one statement may stack.
/// Deeper is refused, so the walk is bounded however long the SQL is.
const MAX_WRAPPING: usize = 4;

/// Catalogs that live in the session, not in the tenant's DuckLake catalog.
const SESSION_CATALOGS: &[&str] = &["temp", "memory"];

/// `Err` naming why when `sql` would change a shared session.
pub(crate) fn refuse_on_shared_session(sql: &str) -> Result<(), ConnectorError> {
    match session_state_reason(sql) {
        None => Ok(()),
        Some(reason) => Err(ConnectorError::query_failed(
            sql,
            format!(
                "{reason} is not allowed here: this connection is shared with other queries \
                 of the workspace, so a statement that changes session state (settings, \
                 temporary objects, transactions, attached databases) would carry over to \
                 them. Qualify names and use literals instead"
            ),
        )),
    }
}

/// Why `sql` may not run on a shared session, or `None` when every statement
/// is safe to share.
pub(crate) fn session_state_reason(sql: &str) -> Option<String> {
    match lex(sql) {
        Ok(statements) => statements.iter().find_map(|t| statement_reason(t, 0)),
        Err(why) => Some(why.to_string()),
    }
}

fn statement_reason(tokens: &[Token], depth: usize) -> Option<String> {
    let at = tokens.iter().position(|t| matches!(t, Token::Word(_)))?;
    let Token::Word(first) = &tokens[at] else {
        unreachable!("position matched a word")
    };
    if WRAPPING_KEYWORDS.contains(&first.as_str()) {
        if depth >= MAX_WRAPPING {
            return Some(format!("{first} nested more than {MAX_WRAPPING} deep"));
        }
        return wrapped_reason(first, skip_word(&tokens[at..]), depth + 1);
    }
    if READ_KEYWORDS.contains(&first.as_str()) {
        return None;
    }
    if WRITE_KEYWORDS.contains(&first.as_str()) {
        return write_reaches_session(&tokens[at..]);
    }
    Some(first.clone())
}

/// `EXPLAIN`, `SUMMARIZE` or `DESCRIBE` followed by `rest`: judged by the
/// statement in `rest` when there is one (`EXPLAIN ANALYZE` runs it). Other
/// than `EXPLAIN`, they may name a relation instead, which only reads.
fn wrapped_reason(keyword: &str, mut rest: &[Token], depth: usize) -> Option<String> {
    if keyword == "EXPLAIN" {
        loop {
            match rest.first() {
                Some(Token::Word(w)) if EXPLAIN_OPTIONS.contains(&w.as_str()) => {
                    rest = skip_word(rest);
                }
                Some(Token::Open) => match skip_group(rest) {
                    Some(tail) => rest = tail,
                    None => return Some("EXPLAIN with unbalanced options".into()),
                },
                _ => break,
            }
        }
    }
    if let Some(Token::Word(next)) = rest.first() {
        let next = next.as_str();
        if READ_KEYWORDS.contains(&next)
            || WRITE_KEYWORDS.contains(&next)
            || WRAPPING_KEYWORDS.contains(&next)
        {
            return statement_reason(rest, depth);
        }
    }
    if keyword != "EXPLAIN" && is_relation_name(rest) {
        return None;
    }
    let next = rest.iter().find_map(|t| match t {
        Token::Word(w) => Some(w.as_str()),
        _ => None,
    });
    Some(match next {
        Some(w) => format!("{keyword} {w}"),
        None => keyword.to_string(),
    })
}

/// `tokens` is just a relation name: words, quoted identifiers and dots.
fn is_relation_name(tokens: &[Token]) -> bool {
    !tokens.is_empty()
        && tokens
            .iter()
            .all(|t| matches!(t, Token::Word(_) | Token::Ident(_) | Token::Dot))
}

/// Drop a bare word: its `Word` and the `Ident` the lexer pairs with it.
fn skip_word(rest: &[Token]) -> &[Token] {
    match rest {
        [Token::Word(_), Token::Ident(_), tail @ ..] | [Token::Word(_), tail @ ..] => tail,
        _ => rest,
    }
}

/// Drop the parenthesised group starting at `rest[0]`; `None` when it never
/// closes.
fn skip_group(rest: &[Token]) -> Option<&[Token]> {
    let mut depth = 0usize;
    for (i, t) in rest.iter().enumerate() {
        match t {
            Token::Open => depth += 1,
            Token::Close => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&rest[i + 1..]);
                }
            }
            _ => {}
        }
    }
    None
}

/// A write that creates a session-scoped object or names a session catalog.
fn write_reaches_session(tokens: &[Token]) -> Option<String> {
    let mut words = tokens.iter().filter_map(|t| match t {
        Token::Word(w) => Some(w.as_str()),
        _ => None,
    });
    if let Some(verb @ ("CREATE" | "DROP")) = words.next() {
        for w in words {
            if SESSION_CREATE_WORDS.contains(&w) {
                return Some(format!("{verb} {w}"));
            }
            if CREATE_OBJECT_TYPES.contains(&w) {
                break;
            }
        }
    }
    tokens.windows(2).find_map(|pair| match pair {
        [Token::Ident(name), Token::Dot] if SESSION_CATALOGS.contains(&name.as_str()) => {
            Some(format!("a write to the session catalog `{name}`"))
        }
        _ => None,
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A bare word, uppercased, so keywords compare the way DuckDB does. The
    /// lexer follows each with its [`Token::Ident`] form.
    Word(String),
    /// A quoted or bare identifier, lowercased.
    Ident(String),
    Dot,
    Open,
    Close,
    Other,
}

/// Split `sql` into statements of tokens, or say why it cannot be split with
/// certainty.
fn lex(sql: &str) -> Result<Vec<Vec<Token>>, &'static str> {
    let b = sql.as_bytes();
    let mut statements = vec![Vec::new()];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let current = statements.last_mut().expect("never empty");
        match c {
            b';' => {
                statements.push(Vec::new());
                i += 1;
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                i = sql[i..].find('\n').map_or(b.len(), |n| i + n + 1);
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let end = sql[i + 2..].find("*/").ok_or("an unterminated comment")?;
                if sql[i + 2..i + 2 + end].contains("/*") {
                    return Err("a nested comment");
                }
                i += 2 + end + 2;
            }
            b'\'' => {
                // `1e'…'`: a number's exponent marker or an `E'…'` prefix?
                // Lexers disagree, so refuse rather than pick one.
                if i >= 2 && matches!(b[i - 1], b'E' | b'e') && b[i - 2].is_ascii_digit() {
                    return Err("an ambiguous string prefix");
                }
                i = skip_single_quoted(b, i + 1, is_escape_string_prefix(b, i))?;
                current.push(Token::Other);
            }
            b'"' => {
                let end = skip_double_quoted(b, i + 1)?;
                let name = sql[i + 1..end - 1].replace("\"\"", "\"").to_lowercase();
                current.push(Token::Ident(name));
                i = end;
            }
            b'$' => match dollar_tag(&sql[i..]) {
                Some(tag) => {
                    let body = i + tag.len();
                    let close = sql[body..]
                        .find(tag)
                        .ok_or("an unterminated dollar-quoted string")?;
                    current.push(Token::Other);
                    i = body + close + tag.len();
                }
                None => {
                    current.push(Token::Other);
                    i += 1;
                }
            },
            b'.' => {
                current.push(Token::Dot);
                i += 1;
            }
            b'(' => {
                current.push(Token::Open);
                i += 1;
            }
            b')' => {
                current.push(Token::Close);
                i += 1;
            }
            _ if is_word_start(c) => {
                let start = i;
                // Only an identifier continues through `$`. A token that
                // starts with a digit and runs into `$` (`1$$`, `1a$$`) splits
                // differently between lexers (number then quote, or number then
                // identifier), so it is refused rather than guessed at.
                let number = c.is_ascii_digit();
                while i < b.len() && (is_word_start(b[i]) || (b[i] == b'$' && !number)) {
                    i += 1;
                }
                if number && b.get(i) == Some(&b'$') {
                    return Err("an ambiguous number/identifier boundary");
                }
                let word = &sql[start..i];
                current.push(Token::Word(word.to_ascii_uppercase()));
                current.push(Token::Ident(word.to_ascii_lowercase()));
            }
            _ if c.is_ascii_whitespace() => i += 1,
            _ => {
                current.push(Token::Other);
                i += 1;
            }
        }
    }
    statements.retain(|s| !s.is_empty());
    Ok(statements)
}

/// A byte that may start a word (an identifier, keyword or number).
fn is_word_start(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

/// A byte that may continue an identifier: DuckDB, like Postgres, allows `$`
/// after an identifier's first character, so `a$$` is one word.
fn is_word_byte(c: u8) -> bool {
    is_word_start(c) || c == b'$'
}

/// The `'` at `quote` opens an `E'…'` string: the word just before it is
/// exactly `E`, by the same word boundaries the lexer uses.
fn is_escape_string_prefix(b: &[u8], quote: usize) -> bool {
    quote >= 1 && matches!(b[quote - 1], b'E' | b'e') && (quote == 1 || !is_word_byte(b[quote - 2]))
}

/// The index just past the closing `'` of a string whose body starts at `i`.
fn skip_single_quoted(b: &[u8], mut i: usize, escapes: bool) -> Result<usize, &'static str> {
    while i < b.len() {
        match b[i] {
            b'\\' if escapes => i += 2,
            b'\'' if b.get(i + 1) == Some(&b'\'') => i += 2,
            b'\'' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err("an unterminated string")
}

fn skip_double_quoted(b: &[u8], mut i: usize) -> Result<usize, &'static str> {
    while i < b.len() {
        match b[i] {
            b'"' if b.get(i + 1) == Some(&b'"') => i += 2,
            b'"' => return Ok(i + 1),
            _ => i += 1,
        }
    }
    Err("an unterminated quoted identifier")
}

/// `$tag$` (or `$$`) at the start of `s`. A tag starts with a letter, `_` or
/// a non-ASCII byte and continues with those or digits, as Postgres's
/// `dolq_start` / `dolq_cont`, so `$1` is a parameter. Only reached for a `$`
/// that does not continue a word.
fn dollar_tag(s: &str) -> Option<&str> {
    let rest = &s[1..];
    let end = rest.find('$')?;
    let tag = rest[..end].as_bytes();
    let starts = tag
        .first()
        .is_none_or(|c| c.is_ascii_alphabetic() || *c == b'_' || *c >= 0x80);
    let continues = tag
        .iter()
        .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c >= 0x80);
    (starts && continues).then(|| &s[..end + 2])
}

#[cfg(test)]
#[path = "session_state_tests.rs"]
mod tests;
