//! Which `ctx.oltp` statements a **sandbox** may send to its own schema on
//! the org's OLTP staging branch (`internal-docs/per-org-oltp-postgres.md` →
//! Sandbox schemas on the staging branch).
//!
//! A sandbox connects as the app's own writer — the role staging uses — with
//! its schema as the only `search_path` entry. Postgres would let that role
//! read and write staging's `app_<writer>` and the app's other sandboxes, so
//! what keeps a sandbox in its schema is the search path, and this fence for
//! a statement that tries to leave it. The branch's own check
//! (`oltp_branch_sql`) has already run: nothing reaching outside the database
//! is sent, and SQL decided only when it runs (`DO`, `CALL`, a function
//! definition) is held. A sandbox statement must then also:
//!
//! - **parse** as Postgres — one that does not is refused, since what it
//!   names cannot be read;
//! - **name no other schema**: a relation qualified by anything but the
//!   sandbox's schema, `pg_catalog` or `information_schema`, and any
//!   identifier or string literal that spells staging's schema or another
//!   sandbox of the app (the schemas this role could reach);
//! - **leave name resolution alone**: `SET` / `RESET` of `search_path`,
//!   `schema`, `role` or `session authorization`, `DISCARD`, and
//!   `set_config` of those, or of a setting that is not a literal;
//! - **resolve a name only from a literal**: `nextval`, `setval`, `currval`,
//!   `table_to_xml*`, `schema_to_xml*` and the `to_reg*` lookups take a string
//!   literal, which the rule above then reads; a cast to a `reg*` type takes
//!   one too; `database_to_xml*`, which reads every schema, is refused.
//!
//! **A static layer, as the branch's is.** A name assembled at run time and
//! handed to some other catalog function (a size, a definition) is not seen:
//! that exposes metadata of the app's own staging schema, not rows. The
//! authority a role per sandbox would give is the gap the docs record.

use std::ops::ControlFlow;

use airhouse::sql_parse::with_parsed;
use sqlparser::ast::{ObjectName, ObjectNamePart, Statement, Visit, Visitor};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::ParserError;
use sqlparser::tokenizer::{Token, Tokenizer};

use super::oltp_branch_sql::{keyword, string_value};
use super::{HeldStatement, HostOp, refused_message};
use oxy_app_core::custom_app_environment::AppEnvironment;

/// `Refuse` fix for a sandbox statement that leaves the sandbox's schema.
pub const SANDBOX_REFUSED_FIX: &str = "a sandbox's OLTP statements run in its own schema on the \
     org's staging branch, with that schema as the only one names resolve in; a statement that \
     names another schema, or changes how names resolve, is not sent. Write table names \
     unqualified";

/// Schemas a statement may name besides the sandbox's own.
const SYSTEM_SCHEMAS: &[&str] = &["pg_catalog", "information_schema"];

/// What `SET` / `RESET` may not touch: each decides which schema or role an
/// unqualified name resolves as. `schema` is `SET SCHEMA 'x'`, an alias for
/// `search_path`; `session` is `SET SESSION AUTHORIZATION`.
const NAME_RESOLUTION_SETTINGS: &[&str] = &["SEARCH_PATH", "SCHEMA", "ROLE", "AUTHORIZATION"];

/// `set_config`'s spelling of the same.
const NAME_RESOLUTION_CONFIGS: &[&str] = &["search_path", "role", "session_authorization"];

/// Functions whose first argument is an object's name, resolved when the
/// statement runs: a string literal there is read by the schema rule; an
/// expression is not, so it is refused.
const NAME_ARGUMENT_FUNCTIONS: &[&str] = &["nextval", "setval", "currval"];
const NAME_ARGUMENT_PREFIXES: &[&str] = &["table_to_xml", "schema_to_xml", "to_reg"];

/// Reads every schema the role can see, whatever it is given.
const EVERY_SCHEMA_PREFIXES: &[&str] = &["database_to_xml"];

/// The object-identifier types a text value is cast to to name an object.
const REG_TYPES: &[&str] = &[
    "regclass",
    "regcollation",
    "regconfig",
    "regdictionary",
    "regnamespace",
    "regoper",
    "regoperator",
    "regproc",
    "regprocedure",
    "regrole",
    "regtype",
];

/// The two schema names the fence is built from: the app's own (staging's
/// copy on the branch) and the sandbox's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SandboxFence {
    app_schema: String,
    own: String,
}

impl SandboxFence {
    /// `app_schema` is `app_<writer>`; `own` the sandbox's schema,
    /// `app_<writer>__<label>`.
    pub fn new(app_schema: impl Into<String>, own: impl Into<String>) -> Self {
        Self {
            app_schema: app_schema.into(),
            own: own.into(),
        }
    }

    /// Whether `name` — already folded as Postgres folds it — is a schema of
    /// this app the sandbox must not name: staging's, or another sandbox's.
    fn is_another_of_the_apps(&self, name: &str) -> bool {
        name != self.own
            && (name == self.app_schema
                || name
                    .strip_prefix(self.app_schema.as_str())
                    .is_some_and(|rest| rest.starts_with("__")))
    }

    fn allows_schema(&self, schema: &str) -> bool {
        schema == self.own || SYSTEM_SCHEMAS.contains(&schema)
    }
}

/// `Ok` when `sql` stays inside the sandbox's schema. See the module docs.
pub fn admit_sandbox_statement(fence: &SandboxFence, sql: &str) -> Result<(), HeldStatement> {
    let dialect = PostgreSqlDialect {};
    let Ok(tokens) = Tokenizer::new(&dialect, sql).tokenize() else {
        return Err(unreadable());
    };
    let tokens: Vec<&Token> = tokens
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    // The token rules first: they name what is wrong with a statement the
    // parser may not know (`SET SCHEMA`), where the parse can only say it
    // could not read it.
    named_schemas(fence, &tokens)?;
    name_resolution(&tokens)?;
    runtime_names(&tokens)?;
    with_parsed(&dialect, sql, |parsed| qualified_relations(fence, parsed))
        .unwrap_or_else(|too_deep| Err(refused("UNCLASSIFIED", too_deep.0)))
}

/// The schema of this app, other than the sandbox's own, that `sql` spells in
/// an identifier or a string — the check a sandbox's migration files get
/// (`custom_apps_migrations::sandbox_schema`), which may define functions and
/// so are not held to the rest of the fence. `Err` for SQL that does not
/// tokenize.
pub fn schema_named_in(fence: &SandboxFence, sql: &str) -> Result<Option<String>, HeldStatement> {
    let Ok(tokens) = Tokenizer::new(&PostgreSqlDialect {}, sql).tokenize() else {
        return Err(unreadable());
    };
    let tokens: Vec<&Token> = tokens.iter().collect();
    Ok(named_schemas(fence, &tokens).err().map(|named| named.table))
}

/// The error a statement refused by the fence throws.
pub fn sandbox_statement_message(op: HostOp, environment: &AppEnvironment, why: &str) -> String {
    let refused = refused_message(op, environment, SANDBOX_REFUSED_FIX);
    format!("{refused} This statement {why}.")
}

/// Every relation the parsed statements name is unqualified, or qualified by
/// the sandbox's schema or a system one. SQL that does not parse is refused.
fn qualified_relations(
    fence: &SandboxFence,
    parsed: Result<Vec<Statement>, ParserError>,
) -> Result<(), HeldStatement> {
    let statements = parsed.map_err(|_| unreadable())?;
    if statements.is_empty() {
        return Err(unreadable());
    }
    let mut relations = Relations { fence };
    match statements.visit(&mut relations) {
        ControlFlow::Break(refusal) => Err(refusal),
        ControlFlow::Continue(()) => Ok(()),
    }
}

struct Relations<'a> {
    fence: &'a SandboxFence,
}

impl Visitor for Relations<'_> {
    type Break = HeldStatement;

    fn pre_visit_relation(&mut self, relation: &ObjectName) -> ControlFlow<HeldStatement> {
        let parts: Option<Vec<String>> = relation.0.iter().map(folded).collect();
        let Some(parts) = parts else {
            return ControlFlow::Break(refused(
                "NAME",
                format!("names {relation} through a function, which cannot be checked"),
            ));
        };
        let schema = match parts.as_slice() {
            [] | [_] => return ControlFlow::Continue(()),
            [schema, _] => schema,
            _ => {
                return ControlFlow::Break(refused(
                    "NAME",
                    format!("names {relation} through a database, not a schema"),
                ));
            }
        };
        if self.fence.allows_schema(schema) {
            return ControlFlow::Continue(());
        }
        ControlFlow::Break(HeldStatement {
            verb: "NAME".to_string(),
            table: relation.to_string(),
            why: format!(
                "names {relation} in schema {schema}, which is not this sandbox's own ({})",
                self.fence.own
            ),
        })
    }
}

/// One part of a name as Postgres reads it: quoted, exactly; unquoted,
/// lower-cased. `None` for a part that is not an identifier.
fn folded(part: &ObjectNamePart) -> Option<String> {
    match part {
        ObjectNamePart::Identifier(ident) if ident.quote_style.is_some() => {
            Some(ident.value.clone())
        }
        ObjectNamePart::Identifier(ident) => Some(ident.value.to_ascii_lowercase()),
        _ => None,
    }
}

/// No identifier, and no string literal, spells a schema of this app other
/// than the sandbox's — wherever it appears: a function's qualifier, a type,
/// `SET SCHEMA`, a name inside a string that Postgres resolves at run time.
fn named_schemas(fence: &SandboxFence, tokens: &[&Token]) -> Result<(), HeldStatement> {
    for token in tokens {
        let named = match token {
            Token::Word(word) if word.quote_style.is_some() => fence
                .is_another_of_the_apps(&word.value)
                .then(|| word.value.clone()),
            Token::Word(word) => {
                let name = word.value.to_ascii_lowercase();
                fence.is_another_of_the_apps(&name).then_some(name)
            }
            other => string_value(other).and_then(|text| schema_in_text(fence, text)),
        };
        if let Some(schema) = named {
            return Err(HeldStatement {
                verb: "NAME".to_string(),
                table: schema.clone(),
                why: format!(
                    "names schema {schema}, which is not this sandbox's own ({})",
                    fence.own
                ),
            });
        }
    }
    Ok(())
}

/// The first schema of this app, other than the sandbox's, that `text` spells
/// as a whole identifier — compared without case, since a name in a string
/// folds when Postgres resolves it.
fn schema_in_text(fence: &SandboxFence, text: &str) -> Option<String> {
    let lowered = text.to_ascii_lowercase();
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(at) = lowered[from..].find(fence.app_schema.as_str()) {
        let start = from + at;
        let starts_a_name = !lowered[..start].chars().next_back().is_some_and(is_ident);
        let len = lowered[start..]
            .find(|c: char| !is_ident(c))
            .unwrap_or(lowered.len() - start);
        let name = &lowered[start..start + len];
        if starts_a_name && fence.is_another_of_the_apps(name) {
            return Some(name.to_string());
        }
        from = start + fence.app_schema.len();
    }
    None
}

/// Nothing changes which schema, or which role, an unqualified name resolves
/// as: `SET` / `RESET` of a name-resolution setting, `DISCARD`, `set_config`.
fn name_resolution(tokens: &[&Token]) -> Result<(), HeldStatement> {
    let mut at_start = true;
    for (i, token) in tokens.iter().enumerate() {
        if at_start
            && let Some(word) = keyword(token)
            && let Some(verb) = session_verb(&word, &tokens[i + 1..])
        {
            return Err(refused(
                &verb,
                format!("runs {verb}, which changes how names resolve in the session"),
            ));
        }
        if let [Token::Word(name), Token::LParen, rest @ ..] = &tokens[i..]
            && name.value.eq_ignore_ascii_case("set_config")
            && !sets_a_harmless_config(rest)
        {
            return Err(refused(
                "SET_CONFIG",
                "calls set_config() on a setting that decides how names resolve, or one that \
                 is not a literal"
                    .to_string(),
            ));
        }
        at_start = matches!(token, Token::SemiColon);
    }
    Ok(())
}

/// The name-resolution verb `word` starts, given the tokens after it. A
/// setting's name is read quoted or not: `SET "search_path" TO x` is the same
/// statement.
fn session_verb(word: &str, rest: &[&Token]) -> Option<String> {
    let following: Vec<String> = rest
        .iter()
        .take(3)
        .filter_map(|t| match t {
            Token::Word(w) => Some(w.value.to_ascii_uppercase()),
            _ => None,
        })
        .collect();
    let touches = |setting: &String| NAME_RESOLUTION_SETTINGS.contains(&setting.as_str());
    match word {
        "DISCARD" => Some("DISCARD".to_string()),
        "RESET" => following
            .first()
            .filter(|w| touches(w) || *w == "ALL" || *w == "SESSION")
            .map(|w| format!("RESET {w}")),
        // `SET [SESSION | LOCAL] <setting>`, `SET SESSION AUTHORIZATION`.
        "SET" => following
            .iter()
            .take_while(|w| touches(w) || *w == "SESSION" || *w == "LOCAL")
            .find(|w| touches(w))
            .map(|w| format!("SET {w}")),
        _ => None,
    }
}

/// `set_config('<setting>', …)` with a literal setting that is not one of
/// [`NAME_RESOLUTION_CONFIGS`].
fn sets_a_harmless_config(arguments: &[&Token]) -> bool {
    match sole_literal(arguments) {
        Some(setting) => !NAME_RESOLUTION_CONFIGS.contains(&setting.to_ascii_lowercase().as_str()),
        None => false,
    }
}

/// The first argument of a call, when it is one string literal and nothing
/// else — `'x'`, or `'x'::regclass` — given the tokens after the call's `(`.
/// `'a' || 'b'` starts with a literal and is an expression.
fn sole_literal<'a>(arguments: &[&'a Token]) -> Option<&'a str> {
    let literal = string_value(arguments.first().copied()?)?;
    let mut after = 1;
    while matches!(arguments.get(after), Some(Token::DoubleColon)) {
        // `::type`, or `::pg_catalog.type`.
        after += match arguments.get(after + 2) {
            Some(Token::Period) => 4,
            _ => 2,
        };
    }
    matches!(arguments.get(after), Some(Token::Comma | Token::RParen)).then_some(literal)
}

/// A name Postgres resolves when the statement runs is given as a string
/// literal — which [`named_schemas`] read — never as an expression.
fn runtime_names(tokens: &[&Token]) -> Result<(), HeldStatement> {
    for (i, token) in tokens.iter().enumerate() {
        if let [Token::Word(name), Token::LParen, rest @ ..] = &tokens[i..] {
            let name = name.value.to_ascii_lowercase();
            if EVERY_SCHEMA_PREFIXES.iter().any(|p| name.starts_with(p)) {
                return Err(refused(
                    "CALL",
                    format!("calls {name}(), which reads every schema of the database"),
                ));
            }
            let takes_a_name = NAME_ARGUMENT_FUNCTIONS.contains(&name.as_str())
                || NAME_ARGUMENT_PREFIXES.iter().any(|p| name.starts_with(p));
            if takes_a_name && sole_literal(rest).is_none() {
                return Err(refused(
                    "CALL",
                    format!("calls {name}() on a name that is not a string literal"),
                ));
            }
        }
        if is_reg_type(token) && casts_an_expression(tokens, i) {
            return Err(refused(
                "CAST",
                "casts an expression to an object-identifier type, naming an object by a \
                 value that is not a string literal"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// `regclass`, `regnamespace`, `regproc`, … — quoted or not.
fn is_reg_type(token: &Token) -> bool {
    matches!(token, Token::Word(word)
        if REG_TYPES.contains(&word.value.to_ascii_lowercase().as_str()))
}

/// Whether the `reg*` type at `tokens[at]` is the target of a cast whose
/// operand is not a string literal: `<x>::regclass`, `CAST(<x> AS regclass)`.
/// `pg_catalog.regclass` is the same type. `::` binds its one left operand,
/// so a literal before it is the whole operand; `CAST(` takes an expression,
/// so the literal must be all there is between the parenthesis and `AS`.
fn casts_an_expression(tokens: &[&Token], at: usize) -> bool {
    let mut before = at;
    if before >= 2
        && matches!(tokens[before - 1], Token::Period)
        && keyword(tokens[before - 2]).as_deref() == Some("PG_CATALOG")
    {
        before -= 2;
    }
    let operand = |back: usize| before.checked_sub(back).map(|i| tokens[i]);
    let literal = operand(2).is_some_and(|t| string_value(t).is_some());
    match operand(1) {
        Some(Token::DoubleColon) => !literal,
        Some(token) if keyword(token).as_deref() == Some("AS") => {
            !(literal && matches!(operand(3), Some(Token::LParen)))
        }
        _ => false,
    }
}

fn unreadable() -> HeldStatement {
    refused(
        "UNCLASSIFIED",
        "does not parse as Postgres, so what it names cannot be checked".to_string(),
    )
}

fn refused(verb: &str, why: String) -> HeldStatement {
    HeldStatement {
        verb: verb.to_string(),
        table: String::new(),
        why,
    }
}

#[cfg(test)]
#[path = "oltp_sandbox_sql_tests.rs"]
mod tests;
