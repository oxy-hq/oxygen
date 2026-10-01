//! Which `ctx.oltp` statements a staging run may send to the org's OLTP
//! **staging branch**: everything, except a call that reaches outside that
//! database, and SQL decided only when it runs (previews P4b).
//!
//! The branch is a copy of production's database (P4a), so a write, a DDL
//! statement or transaction control all run there as asked — that is what
//! makes staging's read-after-write match production's. What a copy cannot
//! contain is a statement that acts outside the database it runs in, which is
//! **refused** ([`NotSent::Refused`]):
//!
//! - **another connection**: `dblink*`;
//! - **server files and programs**: `lo_import` / `lo_export`,
//!   `pg_read_file`, `pg_read_binary_file`, `pg_stat_file`, the `pg_ls_*`
//!   listings, adminpack's `pg_file_*`, and `COPY … TO/FROM PROGRAM` or a file;
//! - **other sessions and the server**: `pg_terminate_backend`,
//!   `pg_cancel_backend`, `pg_reload_conf`, `pg_rotate_logfile`;
//! - **cluster-wide objects**: `CREATE`/`ALTER`/`DROP` of a role, user,
//!   group, database, tablespace, subscription, foreign server or table, or
//!   extension; `ALTER SYSTEM`; `IMPORT FOREIGN SCHEMA`; `LOAD`;
//!   `CHECKPOINT`; and a role-membership `GRANT`/`REVOKE`. On a provider whose
//!   branch shares production's cluster (`LocalProvider`), roles are shared
//!   too: `ALTER ROLE CURRENT_USER PASSWORD …` from staging would lock
//!   production's writer out.
//!
//! A name is read as in `oltp_sql` — case-folded, the last part of a
//! qualified name, quoted or not — and a name spelled with Unicode escapes is
//! refused outright. Strings are read too, because SQL travels in them: a
//! dollar-quoted body, and a string given to `DO`, `AS` or `EXECUTE`, are
//! checked as statements; any other string only for calls, so a stored
//! sentence like "Create user account" is data, not a `CREATE USER`.
//!
//! **SQL decided only when it runs is held** ([`NotSent::Held`]): a `DO`
//! block, `CALL`, `CREATE`/`ALTER FUNCTION|PROCEDURE|ROUTINE`, and a call to
//! a function that runs SQL given as text (`query_to_xml*`, `ts_stat`,
//! `ts_rewrite`). No list can read them: `DO $$ BEGIN EXECUTE 'ALTER' ||
//! ' ROLE CURRENT_USER PASSWORD …'; END $$`, or the same through
//! `format('%s ROLE …', 'ALTER')`, names nothing above until it runs. A
//! function or procedure ships as a migration instead, and calling one that
//! exists is sent.
//!
//! **This is a static layer, not the authority.** What stops a function that
//! already exists in the schema is what stops it in production: the app's
//! writer role holds none of `pg_read_server_files`, `pg_write_server_files`
//! or `pg_execute_server_program`, cannot create an untrusted extension, and
//! owns nothing outside its schema.
//!
//! **A local branch is not an isolation boundary for role-level effects of
//! app-defined functions.** `LocalProvider` cuts a branch as a sibling
//! database on production's own cluster, and roles are cluster-wide: the
//! writer on the branch *is* production's writer. A function already in the
//! app's schema (from a migration, or copied from production) that changes
//! its role — `ALTER ROLE CURRENT_USER …`, `pg_terminate_backend` on its own
//! sessions — acts on production when a staging run calls it. That provider is
//! for dev and CI loopback only; Neon re-mints every role per branch, so there
//! the same call touches only the branch.

use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::tokenizer::{Token, Tokenizer};

use super::oltp_sql::{SQL_TEXT_FUNCTIONS, SQL_TEXT_PREFIXES, listed, names_a_unicode_escape};
use super::{HELD_LABEL, HeldStatement, HostOp, refused_message};

/// Built-in functions that reach another connection, the server's files, or
/// other sessions.
const OUTSIDE_FUNCTIONS: &[&str] = &[
    "lo_import",
    "lo_export",
    "pg_read_file",
    "pg_read_binary_file",
    "pg_stat_file",
    "pg_logdir_ls",
    "pg_terminate_backend",
    "pg_cancel_backend",
    "pg_reload_conf",
    "pg_rotate_logfile",
];

/// The same, by family: `dblink`, `dblink_exec`, …; `pg_ls_dir`,
/// `pg_ls_logdir`, …; adminpack's `pg_file_write`, `pg_file_unlink`, ….
const OUTSIDE_PREFIXES: &[&str] = &["dblink", "pg_ls_", "pg_file_"];

/// What `CREATE`/`ALTER`/`DROP` may name that lives in the cluster, or
/// connects out of it, rather than in the database.
const CLUSTER_OBJECTS: &[&str] = &[
    "ROLE",
    "USER",
    "GROUP",
    "DATABASE",
    "TABLESPACE",
    "SYSTEM",
    "SUBSCRIPTION",
    "SERVER",
    "FOREIGN",
    "EXTENSION",
];

/// Statements that act on the server whatever follows them.
const CLUSTER_STATEMENTS: &[&str] = &["IMPORT", "LOAD", "CHECKPOINT"];

/// How deep strings holding SQL may nest before the statement is refused as
/// unreadable.
const MAX_DEPTH: usize = 4;

/// Whether a string is read for statements or only for calls.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    /// SQL that runs as statements: the top level, a function or `DO` body.
    Code,
    /// A value: read only for calls, which run wherever they appear.
    Text,
}

/// What `CREATE`/`ALTER` may name that defines code run later.
const CODE_OBJECTS: &[&str] = &["FUNCTION", "PROCEDURE", "ROUTINE"];

/// Why a statement is not sent to the staging branch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotSent {
    /// It reaches outside the database: refused, `EnvironmentRefused`.
    Refused(HeldStatement),
    /// Its SQL is decided only when it runs, so nothing can check it: held,
    /// `HeldInStaging`.
    Held(HeldStatement),
}

/// `Ok` when `sql` may be sent to the staging branch: it reaches nothing
/// outside that database, and runs no SQL decided only when it runs.
pub fn admit_branch_statement(sql: &str) -> Result<(), NotSent> {
    scan(sql, Scope::Code, 0).map_err(NotSent::Refused)?;
    runs_unread_code(sql).map_err(NotSent::Held)
}

/// The error a statement held on the branch throws.
pub fn branch_held_statement_message(op: HostOp, why: &str) -> String {
    format!(
        "{HELD_LABEL}: ctx.{op} was not performed. On the org's staging branch a statement \
         whose SQL is decided only when it runs — a DO block, CALL, a function or procedure \
         definition, or a call that runs SQL given as text — is held, because nothing can \
         check what it will do. This statement {why}. Ship functions and procedures as \
         migrations; calling one that exists is sent.",
        op = op.name(),
    )
}

/// A statement whose SQL is decided only when it runs: `DO`, `CALL`,
/// `CREATE [OR REPLACE] | ALTER FUNCTION|PROCEDURE|ROUTINE` at the head of a
/// statement, or a call to a function that runs SQL given as text anywhere.
/// `sql` has already tokenized (`scan`).
fn runs_unread_code(sql: &str) -> Result<(), HeldStatement> {
    let Ok(tokens) = Tokenizer::new(&PostgreSqlDialect {}, sql).tokenize() else {
        return Ok(());
    };
    let tokens: Vec<&Token> = tokens
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    let mut at_start = true;
    for (i, token) in tokens.iter().enumerate() {
        let word = keyword(token);
        if at_start && let Some(verb) = word.as_deref().and_then(|w| code_verb(w, &tokens[i + 1..]))
        {
            return Err(outside(
                &verb,
                "",
                format!("runs {verb}, whose SQL is decided only when it runs"),
            ));
        }
        if let [Token::Word(name), Token::LParen, ..] = &tokens[i..] {
            let name = name.value.to_ascii_lowercase();
            if listed(&name, SQL_TEXT_FUNCTIONS, SQL_TEXT_PREFIXES) {
                return Err(outside(
                    "CALL",
                    &name,
                    format!("calls {name}(), which runs SQL given as text"),
                ));
            }
        }
        at_start = matches!(token, Token::SemiColon);
    }
    Ok(())
}

/// The code-running verb `word` starts, given the tokens after it.
fn code_verb(word: &str, rest: &[&Token]) -> Option<String> {
    let words: Vec<String> = rest.iter().take(3).filter_map(|t| keyword(t)).collect();
    let object = match (word, words.first().map(String::as_str)) {
        ("DO" | "CALL", _) => return Some(word.to_string()),
        ("CREATE", Some("OR")) => words.get(2),
        ("CREATE" | "ALTER", _) => words.first(),
        _ => None,
    }?;
    CODE_OBJECTS
        .contains(&object.as_str())
        .then(|| format!("{word} {object}"))
}

/// The error a statement refused on the branch throws: the op's refusal,
/// then what the statement does.
pub fn branch_statement_message(
    op: HostOp,
    environment: &oxy_app_core::custom_app_environment::AppEnvironment,
    why: &str,
) -> String {
    let refused = refused_message(op, environment, super::BRANCH_REFUSED_FIX);
    format!("{refused} This statement {why}.")
}

fn scan(sql: &str, scope: Scope, depth: usize) -> Result<(), HeldStatement> {
    if depth > MAX_DEPTH {
        return Err(outside(
            "NESTED",
            "",
            format!("nests SQL in strings more than {MAX_DEPTH} deep, so it cannot be checked"),
        ));
    }
    let Ok(tokens) = Tokenizer::new(&PostgreSqlDialect {}, sql).tokenize() else {
        return match scope {
            // A value that is not SQL is just a value.
            Scope::Text => Ok(()),
            Scope::Code => Err(outside(
                "UNCLASSIFIED",
                "",
                "could not be read for what it calls".to_string(),
            )),
        };
    };
    let tokens: Vec<&Token> = tokens
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    if names_a_unicode_escape(&tokens) {
        return Err(outside(
            "UNCLASSIFIED",
            "",
            "spells a name with Unicode escapes (U&\"…\"), so what it names cannot be checked"
                .to_string(),
        ));
    }
    outside_call(&tokens)?;
    if scope == Scope::Code {
        cluster_statement(&tokens)?;
    }
    for (i, token) in tokens.iter().enumerate() {
        if let Some((text, nested)) = string_body(&tokens, i, token) {
            scan(text, nested, depth + 1)?;
        }
    }
    Ok(())
}

/// A call to a function that reaches outside the database.
fn outside_call(tokens: &[&Token]) -> Result<(), HeldStatement> {
    for pair in tokens.windows(2) {
        if let [Token::Word(word), Token::LParen] = pair {
            let name = word.value.to_ascii_lowercase();
            if listed(&name, OUTSIDE_FUNCTIONS, OUTSIDE_PREFIXES) {
                return Err(outside(
                    "CALL",
                    &name,
                    format!("calls {name}(), which reaches outside the staging branch"),
                ));
            }
        }
    }
    Ok(())
}

/// A statement, at the head of a statement or of a PL/pgSQL block, that acts
/// on the cluster; or a `COPY` to or from a program or a server file.
///
/// A PL/pgSQL block head (after `BEGIN`, `THEN`, `ELSE`, `LOOP`) is also where
/// SQL's `CASE` puts an expression, so only the two-word verbs are looked for
/// there: `THEN load` is a column named `load`, `THEN ALTER ROLE` is not.
fn cluster_statement(tokens: &[&Token]) -> Result<(), HeldStatement> {
    let (mut at_start, mut at_head) = (true, true);
    for (i, token) in tokens.iter().enumerate() {
        let word = keyword(token);
        if at_head && let Some(word) = word.as_deref() {
            let rest = &tokens[i + 1..];
            if let Some(verb) = acts_on_cluster(word, rest, at_start) {
                return Err(outside(
                    &verb,
                    "",
                    format!("runs {verb}, which acts on the server, not the staging branch"),
                ));
            }
            if word == "COPY" && copies_outside(rest) {
                return Err(outside(
                    "COPY",
                    "",
                    "copies to or from a program or a server file".to_string(),
                ));
            }
        }
        at_start = matches!(token, Token::SemiColon);
        at_head = at_start || matches!(word.as_deref(), Some("BEGIN" | "THEN" | "ELSE" | "LOOP"));
    }
    Ok(())
}

/// The cluster verb `word` starts, given the tokens after it. The one-word
/// verbs count only at the start of a statement (`at_start`).
fn acts_on_cluster(word: &str, rest: &[&Token], at_start: bool) -> Option<String> {
    let next = rest.first().and_then(|t| keyword(t));
    match (word, next.as_deref()) {
        ("CREATE" | "ALTER" | "DROP", Some(object)) if CLUSTER_OBJECTS.contains(&object) => {
            Some(format!("{word} {object}"))
        }
        (word, _) if at_start && CLUSTER_STATEMENTS.contains(&word) => Some(word.to_string()),
        ("GRANT", _) if !names_before(rest, "ON", "TO") => Some("GRANT (role membership)".into()),
        ("REVOKE", _) if !names_before(rest, "ON", "FROM") => {
            Some("REVOKE (role membership)".into())
        }
        _ => None,
    }
}

/// Whether `first` appears before `then` (or the statement's end) — `GRANT …
/// ON t TO r` is a privilege, `GRANT a TO r` a role membership.
fn names_before(rest: &[&Token], first: &str, then: &str) -> bool {
    for token in rest {
        if matches!(token, Token::SemiColon) {
            return false;
        }
        match keyword(token).as_deref() {
            Some(word) if word == first => return true,
            Some(word) if word == then => return false,
            _ => {}
        }
    }
    false
}

/// `COPY … TO/FROM PROGRAM '…'` or `COPY … TO/FROM '<file>'`. `STDIN` and
/// `STDOUT` stay inside the connection.
fn copies_outside(rest: &[&Token]) -> bool {
    let statement = rest
        .iter()
        .take_while(|t| !matches!(t, Token::SemiColon))
        .collect::<Vec<_>>();
    statement.windows(2).any(|pair| {
        matches!(keyword(pair[0]).as_deref(), Some("TO" | "FROM"))
            && (keyword(pair[1]).as_deref() == Some("PROGRAM") || string_value(pair[1]).is_some())
    })
}

/// A string in `tokens[i]` worth reading, and how: a dollar-quoted body, or a
/// string given to `DO`, `AS` or `EXECUTE`, as statements; any other string
/// for calls only.
fn string_body<'a>(tokens: &[&Token], i: usize, token: &'a Token) -> Option<(&'a str, Scope)> {
    if let Token::DollarQuotedString(body) = token {
        return Some((body.value.as_str(), Scope::Code));
    }
    let value = string_value(token)?;
    let takes_code = i > 0
        && matches!(
            keyword(tokens[i - 1]).as_deref(),
            Some("DO" | "AS" | "EXECUTE")
        );
    Some((value, if takes_code { Scope::Code } else { Scope::Text }))
}

/// The value of a string literal, escapes already decoded by the tokenizer.
fn string_value(token: &Token) -> Option<&str> {
    match token {
        Token::SingleQuotedString(s)
        | Token::EscapedStringLiteral(s)
        | Token::NationalStringLiteral(s)
        | Token::UnicodeStringLiteral(s) => Some(s.as_str()),
        Token::DollarQuotedString(body) => Some(body.value.as_str()),
        _ => None,
    }
}

/// An unquoted word, upper-cased: a keyword or a bare name.
fn keyword(token: &Token) -> Option<String> {
    match token {
        Token::Word(w) if w.quote_style.is_none() => Some(w.value.to_ascii_uppercase()),
        _ => None,
    }
}

fn outside(verb: &str, table: &str, why: String) -> HeldStatement {
    HeldStatement {
        verb: verb.to_string(),
        table: table.to_string(),
        why,
    }
}

#[cfg(test)]
#[path = "oltp_branch_sql_tests.rs"]
mod tests;
