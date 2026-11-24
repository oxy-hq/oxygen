//! How an OLTP statement is admitted outside production — on the org's
//! staging branch, or held on production (`env_guard` has the rest of the
//! mechanism; the decisions are `env_policy`'s).
//!
//! - **On the branch** (previews P4b): every statement runs as asked except
//!   one that reaches outside that database (refused) or whose SQL is decided
//!   only when it runs — `DO`, `CALL`, a function definition (held)
//!   (`env_policy::admit_branch_statement`); either is listed in the held
//!   row. A `ctx.oltp.tx` handle opened there runs its statements and its
//!   commit there.
//! - **In a sandbox's own schema on the branch**: the same, and a statement
//!   that names another schema, or changes how names resolve, is refused too
//!   (`env_policy::admit_sandbox_statement`). While that schema is not ready
//!   every OLTP op is refused before anything connects
//!   ([`ProjectFunctionHost::refuse_unready_sandbox`]).
//! - **On a sandbox's own branch** (`OltpHome::EnvironmentBranch`): the staging
//!   branch's rule, unchanged — it is also a copy of production's database
//!   (or an empty one), so a statement that reaches outside it is refused.
//!   While the branch is not ready every OLTP op is refused, as for an
//!   unready schema ([`ProjectFunctionHost::refuse_unready_sandbox`]).
//! - **Held on production** (no branch): a statement is sent only when it is
//!   one read calling no side-effect function
//!   (`env_policy::admit_oltp_statement`), on a session whose
//!   `default_transaction_read_only` is on, inside a `READ ONLY` transaction —
//!   three layers, so a function the app defined that writes is still refused
//!   by Postgres. Not one that notifies or takes an advisory lock: `READ ONLY`
//!   allows both (see `env_policy::oltp_sql`). Each held entry, and the error,
//!   names how to get a branch.
//! - **An event publish** goes where an OLTP write would: production's store,
//!   or the branch the run — or, on a handle, its `begin` — was admitted to.
//!   Held (with how to get a branch) or refused anywhere else, before
//!   anything connects ([`ProjectFunctionHost::admit_publish`]), so a
//!   non-production event never lands in production's outbox.
//!
//! **In every environment**, production included, a statement naming one of
//! the platform's schemas — the events schema `oxy_events`, or the change
//! stream's `oxy_changes` — or setting `search_path` from a value built when
//! it runs, is refused before it is sent ([`refuse_platform_schema`], which
//! says what it cannot see): apps publish through `ctx.events.publish`, and
//! the platform captures their changes for them.

use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::tokenizer::{Token, Tokenizer};

use super::super::env_policy::{self, Decision, HostOp, Target};
use super::super::host_call_attrs::QuerySummary;
use super::env_guard::{HeldTarget, holds};
use super::*;

/// The platform's events schema (`oxy_oltp::platform::EVENTS_SCHEMA`).
const PLATFORM_EVENTS_SCHEMA: &str = oxy_oltp::platform::EVENTS_SCHEMA;

/// The platform's change-stream schema (`oxy_oltp::platform::CHANGES_SCHEMA`,
/// platform step 8): the capture log, and the prefix of the settings the host
/// signs each production transaction with (`oltp_changes::provenance`).
const PLATFORM_CHANGES_SCHEMA: &str = oxy_oltp::platform::CHANGES_SCHEMA;

/// What `ctx.oltp` SQL naming [`PLATFORM_EVENTS_SCHEMA`] is refused with, in
/// every environment.
pub(super) const PLATFORM_SCHEMA_REFUSAL: &str =
    "OltpPlatformSchema: `oxy_events` belongs to the platform; publish with ctx.events.publish";

/// What `ctx.oltp` SQL naming [`PLATFORM_CHANGES_SCHEMA`] — its log, or one
/// of the settings the host signs a transaction with — is refused with, in
/// every environment.
pub(super) const CHANGES_SCHEMA_REFUSAL: &str = "OltpPlatformSchema: `oxy_changes` belongs to \
     the platform; it records the changes to the tables oltpChanges declares, and an app's SQL \
     may not read, write or set it";

/// Every schema of the platform's an app's SQL may not name, each with what
/// naming it is refused with. The first a statement names decides.
const PLATFORM_SCHEMAS: [(&str, &str); 2] = [
    (PLATFORM_EVENTS_SCHEMA, PLATFORM_SCHEMA_REFUSAL),
    (PLATFORM_CHANGES_SCHEMA, CHANGES_SCHEMA_REFUSAL),
];

/// What a Unicode-escaped identifier is refused with: it can spell any name.
const UNICODE_IDENTIFIER_REFUSAL: &str = "OltpPlatformSchema: a Unicode-escaped identifier \
     (U&\"…\") could spell `oxy_events` or `oxy_changes`, which belong to the platform; write \
     the name plainly";

/// What setting `search_path` from a value built when the statement runs is
/// refused with: it could name a platform schema, and an unqualified
/// `publish(...)` would then reach it.
const SEARCH_PATH_REFUSAL: &str = "OltpPlatformSchema: `search_path` may be set only to plain \
     names or plain string literals, which are checked for `oxy_events` and `oxy_changes`; this \
     statement sets it from a value built when it runs";

/// `Err` when `sql` reaches one of the platform's schemas
/// ([`PLATFORM_SCHEMAS`]) in a way that can be read off the statement, in
/// every environment, before it is sent (after the environment's own
/// admission, so a statement staging holds is reported held). Refused:
///
/// - an identifier `oxy_events` or `oxy_changes` — unquoted in any case, as
///   Postgres folds it, or double-quoted exactly — or the name spelled whole
///   inside a string or a dollar-quoted body, where Postgres may resolve it
///   when the statement runs (`DO … EXECUTE '…'`), or which names one of the
///   `oxy_changes.*` settings (`set_config('oxy_changes.actor', …)`);
///   comments say nothing;
/// - any Unicode-escaped identifier `U&"…"`, which can spell any name;
/// - changing `search_path` to anything but plain names and plain string
///   literals: `set_config` whose setting is not a plain literal, or that sets
///   `search_path` to anything but one; a statement naming `search_path` that
///   carries an escape, Unicode or dollar-quoted string or a backslash; and
///   `UPDATE pg_settings`, which is `SET` by another name.
///
/// SQL the tokenizer cannot read is searched as text for the same names.
///
/// **What it cannot see** is SQL built while it runs: a `DO` block, or a
/// function the app defined (in its migrations, or with `CREATE FUNCTION`
/// here), that executes concatenated text or sets `search_path` from it. The
/// writer holds `EXECUTE` on `oxy_events.publish`, so such SQL can write an
/// outbox row. No lexical rule closes that — the app writes its own functions
/// — and refusing `DO` alone would close nothing while changing what
/// production sends today. What holds there: the definer function records the
/// calling writer role and refuses a depth outside 0–8, a delay outside
/// 0–86 400 s, a key outside 1–200 characters, data over 64 KiB as `jsonb`
/// text and a provenance MAC that is not 32 bytes (platform steps 5 and 6).
/// And such a row is never delivered (R25(c)): the host signs every row it
/// writes, after every check this SQL skips — the function's own
/// `events.publish` list, the org's `app_events` flag, schema validation of
/// the data, the actor, root and causation — with a key that never enters the
/// isolate or the store (`app_events::provenance`), and the relay quarantines
/// a row whose MAC does not match its fields (`unverified_provenance`),
/// keeping its body. **What such SQL can still do:** fill the outbox with
/// rows the relay stores as quarantined events, within the definer's bounds,
/// and pre-empt one of its own app's events by writing that id first (the
/// host's row is then the `ON CONFLICT` no-op); both harm the app alone.
///
/// For `oxy_changes`, such SQL can set the three settings the host signs a
/// transaction with, at any point in it. Nothing lexical holds there either,
/// and nothing needs to: no writer holds any grant on the schema, and the
/// MAC binds the settings to the transaction's own id
/// (`oltp_changes::provenance`), so a value the app wrote ships as
/// unverified.
pub(super) fn refuse_platform_schema(sql: &str) -> Result<(), String> {
    let Ok(tokens) = Tokenizer::new(&PostgreSqlDialect {}, sql).tokenize() else {
        return refuse_in_text(sql);
    };
    for (schema, refusal) in PLATFORM_SCHEMAS {
        if tokens.iter().any(|token| names_schema(token, schema)) {
            return Err(refusal.to_string());
        }
    }
    if has_unicode_identifier(&tokens) {
        return Err(UNICODE_IDENTIFIER_REFUSAL.to_string());
    }
    let code: Vec<&Token> = tokens
        .iter()
        .filter(|t| !matches!(t, Token::Whitespace(_)))
        .collect();
    if sets_search_path_when_it_runs(&code) {
        return Err(SEARCH_PATH_REFUSAL.to_string());
    }
    Ok(())
}

/// Whether `token` names the platform schema `schema` or spells it in a
/// string.
fn names_schema(token: &Token, schema: &str) -> bool {
    match token {
        Token::Word(word) if word.quote_style.is_some() => word.value == schema,
        Token::Word(word) => word.value.eq_ignore_ascii_case(schema),
        Token::DollarQuotedString(body) => spells_schema(&body.value, schema),
        other => string_text(other).is_some_and(|text| spells_schema(text, schema)),
    }
}

/// The text of a string token of any kind; `None` for anything else.
fn string_text(token: &Token) -> Option<&str> {
    match token {
        Token::SingleQuotedString(s)
        | Token::DoubleQuotedString(s)
        | Token::EscapedStringLiteral(s)
        | Token::NationalStringLiteral(s)
        | Token::UnicodeStringLiteral(s)
        | Token::HexStringLiteral(s)
        | Token::SingleQuotedByteStringLiteral(s)
        | Token::DoubleQuotedByteStringLiteral(s)
        | Token::SingleQuotedRawStringLiteral(s)
        | Token::DoubleQuotedRawStringLiteral(s)
        | Token::TripleSingleQuotedString(s)
        | Token::TripleDoubleQuotedString(s) => Some(s),
        Token::DollarQuotedString(body) => Some(&body.value),
        _ => None,
    }
}

/// `U&"…"`: an unquoted `U`, then `&`, then a double-quoted identifier, with
/// nothing between them (Postgres allows nothing there).
fn has_unicode_identifier(tokens: &[Token]) -> bool {
    tokens.windows(3).any(|w| {
        matches!(&w[0], Token::Word(u) if u.quote_style.is_none() && u.value.eq_ignore_ascii_case("u"))
            && w[1] == Token::Ampersand
            && matches!(&w[2], Token::Word(q) if q.quote_style == Some('"'))
    })
}

/// `name` as Postgres resolves the identifier `word`.
fn folded(word: &sqlparser::tokenizer::Word) -> String {
    match word.quote_style {
        Some(_) => word.value.clone(),
        None => word.value.to_ascii_lowercase(),
    }
}

/// A plain string literal: `'…'` with no backslash, which every
/// `standard_conforming_strings` setting reads as written.
fn plain_literal(token: &Token) -> Option<&str> {
    match token {
        Token::SingleQuotedString(s) if !s.contains('\\') => Some(s),
        _ => None,
    }
}

/// Whether the statement may set `search_path` to a value built when it runs
/// (`code` is the tokens without whitespace and comments). See
/// [`refuse_platform_schema`].
fn sets_search_path_when_it_runs(code: &[&Token]) -> bool {
    let word = |t: &Token, name: &str| matches!(t, Token::Word(w) if folded(w) == name);
    for (at, token) in code.iter().enumerate() {
        if word(token, "set_config") && code.get(at + 1) == Some(&&Token::LParen) {
            let args = call_arguments(&code[at + 2..]);
            let setting = match args.first() {
                Some([only]) => plain_literal(only),
                _ => None,
            };
            let Some(setting) = setting else {
                return true;
            };
            if setting.trim().eq_ignore_ascii_case("search_path")
                && !matches!(args.get(1), Some([value]) if plain_literal(value).is_some())
            {
                return true;
            }
        }
    }
    if code.iter().any(|t| word(t, "search_path"))
        && code
            .iter()
            .any(|t| string_text(t).is_some() && plain_literal(t).is_none())
    {
        return true;
    }
    code.iter().any(|t| word(t, "pg_settings")) && code.iter().any(|t| word(t, "update"))
}

/// The top-level arguments of a call whose `(` has just been read, each as
/// its tokens, up to the matching `)` (or the end of the statement).
fn call_arguments<'a>(code: &'a [&'a Token]) -> Vec<&'a [&'a Token]> {
    let mut arguments = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (at, token) in code.iter().enumerate() {
        match token {
            Token::LParen | Token::LBracket => depth += 1,
            Token::RParen | Token::RBracket if depth > 0 => depth -= 1,
            Token::RParen => {
                arguments.push(&code[start..at]);
                return arguments;
            }
            Token::Comma if depth == 0 => {
                arguments.push(&code[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    arguments.push(&code[start..]);
    arguments
}

/// [`refuse_platform_schema`] for SQL the tokenizer cannot read: the names
/// it would have refused, as text.
fn refuse_in_text(sql: &str) -> Result<(), String> {
    let lowered = sql.to_ascii_lowercase();
    if let Some((_, refusal)) = PLATFORM_SCHEMAS
        .iter()
        .find(|(schema, _)| spells_schema(sql, schema))
    {
        Err(refusal.to_string())
    } else if lowered.contains("u&\"") {
        Err(UNICODE_IDENTIFIER_REFUSAL.to_string())
    } else if ["set_config", "search_path", "pg_settings"]
        .iter()
        .any(|name| lowered.contains(name))
    {
        Err(SEARCH_PATH_REFUSAL.to_string())
    } else {
        Ok(())
    }
}

/// Whether `text` spells the platform schema `schema` as a whole name,
/// without case.
fn spells_schema(text: &str, schema: &str) -> bool {
    let lowered = text.to_ascii_lowercase();
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$';
    lowered.match_indices(schema).any(|(at, name)| {
        let before = lowered[..at].chars().next_back();
        let after = lowered[at + name.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Appended to an OLTP DSN's `options` outside production: every transaction
/// the session opens is read-only unless a statement lifts it — which the
/// statement classifier never sends.
const READ_ONLY_SESSION: &str = "-cdefault_transaction_read_only%3Don";

impl ProjectFunctionHost {
    /// `Ok` when an OLTP statement may be sent: always in production; on the
    /// org's staging branch, anything that stays inside that database;
    /// otherwise, only one read calling no side-effect function. A held or
    /// refused statement is logged and never reaches the database.
    /// `opened_into` is where a `ctx.oltp.tx` handle's `begin` went (`None`
    /// for a one-shot `ctx.oltp` call, decided by the op).
    pub(super) async fn admit_statement(
        &self,
        op: HostOp,
        opened_into: Option<Target>,
        sql: &str,
        namespace: &str,
    ) -> Result<(), String> {
        let decided = self.policy.decide_on_handle(op, opened_into);
        if decided == Decision::Isolate(Target::OltpBranch) {
            return self.admit_on_oltp_branch(op, sql, namespace).await;
        }
        self.refuse_unready_sandbox(op, namespace, "STATEMENT")
            .await?;
        if !holds(decided) {
            return Ok(());
        }
        let Err(held) = env_policy::admit_oltp_statement(sql) else {
            return Ok(());
        };
        self.note_held_oltp(op, ("oltp", namespace, &held.verb, &held.table))
            .await;
        Err(self.held_oltp_message(op, &held.why))
    }

    /// A statement bound for the org's staging branch: sent unless it
    /// reaches outside that database (refused) or runs SQL decided only when
    /// it runs (held) — `env_policy::admit_branch_statement`. Either is logged
    /// in the held row and never reaches the branch.
    async fn admit_on_oltp_branch(
        &self,
        op: HostOp,
        sql: &str,
        namespace: &str,
    ) -> Result<(), String> {
        let own_branch = matches!(
            self.policy.oltp_home(),
            env_policy::OltpHome::EnvironmentBranch(_)
        );
        let (statement, message) = match env_policy::admit_branch_statement(sql) {
            Ok(()) => return self.admit_in_sandbox_schema(op, sql, namespace).await,
            Err(env_policy::NotSent::Refused(statement)) => {
                let environment = self.policy.environment();
                let message = match own_branch {
                    true => {
                        env_policy::env_branch_statement_message(op, environment, &statement.why)
                    }
                    false => env_policy::branch_statement_message(op, environment, &statement.why),
                };
                (statement, message)
            }
            Err(env_policy::NotSent::Held(statement)) => {
                let message = match own_branch {
                    true => env_policy::env_branch_held_statement_message(op, &statement.why),
                    false => env_policy::branch_held_statement_message(op, &statement.why),
                };
                (statement, message)
            }
        };
        self.note_held(op, ("oltp", namespace, &statement.verb, &statement.table))
            .await;
        Err(message)
    }

    /// A statement the branch admits, bound for a **sandbox's** schema there:
    /// sent unless it names another schema or changes how names resolve
    /// (`env_policy::admit_sandbox_statement`). Staging, which runs in the
    /// app's own schema, has no such fence.
    async fn admit_in_sandbox_schema(
        &self,
        op: HostOp,
        sql: &str,
        namespace: &str,
    ) -> Result<(), String> {
        let env_policy::OltpHome::SandboxSchema(home) = self.policy.oltp_home() else {
            return Ok(());
        };
        let fence = env_policy::SandboxFence::new(home.schema.app_schema(), home.schema.name());
        let Err(statement) = env_policy::admit_sandbox_statement(&fence, sql) else {
            return Ok(());
        };
        self.note_held(op, ("oltp", namespace, &statement.verb, &statement.table))
            .await;
        Err(env_policy::sandbox_statement_message(
            op,
            self.policy.environment(),
            &statement.why,
        ))
    }

    /// Where the environment lets an event publish go, decided before
    /// anything connects. `decided` is the op's decision — the handle's, for
    /// a publish on a `ctx.oltp.tx` handle (`EnvPolicy::decide_on_handle`).
    /// `Ok` with the outbox to write: production's store, or the OLTP branch
    /// the run (or the handle) is on (`EnvPolicy::outbox_target`); held or
    /// refused, logged, otherwise — never written to production's store from
    /// outside production. A sandbox on an OLTP branch of its own writes that
    /// branch, which the relay reads.
    pub(super) async fn admit_publish(
        &self,
        op: HostOp,
        decided: Decision,
        name: &str,
    ) -> Result<oxy_oltp::events::OutboxTarget, String> {
        let target: HeldTarget<'_> = ("oltp", PLATFORM_EVENTS_SCHEMA, "PUBLISH", name);
        let refuse = |fix| Decision::Refuse { fix };
        let decided = match decided {
            Decision::Allow | Decision::Isolate(Target::OltpBranch) => {
                match self.policy.outbox_target() {
                    Ok(outbox) => return Ok(outbox),
                    Err(fix) => refuse(fix),
                }
            }
            other => other,
        };
        match decided {
            Decision::Hold => {
                self.note_held_oltp(op, target).await;
                Err(self.with_branch_note(self.held_error(op)))
            }
            // A refusal in the policy's words; any other home is not one a
            // publish can be written to, and is refused rather than run.
            other => match self.admit_decided(op, other, target).await {
                Err(refused) => Err(refused),
                Ok(()) => Err(format!("ctx.{} has no outbox to write", op.name())),
            },
        }
    }

    /// `Err` when the policy refuses `op` outright — a sandbox whose own
    /// schema on the org's staging branch is not ready. Noted in the held
    /// row; nothing connects.
    pub(super) async fn refuse_unready_sandbox(
        &self,
        op: HostOp,
        namespace: &str,
        verb: &str,
    ) -> Result<(), String> {
        let Decision::Refuse { fix } = self.policy.decide(op) else {
            return Ok(());
        };
        self.note_held(op, ("oltp", namespace, verb, "")).await;
        Err(env_policy::refused_message(
            op,
            self.policy.environment(),
            fix,
        ))
    }

    /// Where a `ctx.oltp.tx` handle opens into: the staging branch when the
    /// policy isolates `tx.begin_oltp` there, else `None` (as asked).
    pub(super) fn oltp_opened_into(&self) -> Option<Target> {
        match self.policy.decide(HostOp::TxBeginOltp) {
            Decision::Isolate(target) => Some(target),
            _ => None,
        }
    }

    /// Whether this is a staging run whose org has no OLTP staging branch —
    /// the one case a held OLTP call names how to get one.
    fn lacks_a_branch(&self) -> bool {
        !self.policy.is_production() && *self.policy.oltp_home() == env_policy::OltpHome::Production
    }

    /// [`Self::note_held`] for a held OLTP call, carrying why it could not
    /// run on a staging copy instead: the org has no staging branch.
    pub(super) async fn note_held_oltp(&self, op: HostOp, target: HeldTarget<'_>) {
        let note = self.lacks_a_branch().then_some(env_policy::NO_BRANCH_NOTE);
        self.note_held_with(op, target, note).await;
    }

    /// What a held OLTP statement throws: the op's hold, what the statement
    /// was, and — in staging with no branch — how to get one.
    fn held_oltp_message(&self, op: HostOp, why: &str) -> String {
        let held = env_policy::held_statement_message(&self.held_error(op), why);
        self.with_branch_note(held)
    }

    /// `message`, followed by [`env_policy::NO_BRANCH_NOTE`] when this is a
    /// staging run whose org has no OLTP staging branch.
    fn with_branch_note(&self, message: String) -> String {
        if !self.lacks_a_branch() {
            return message;
        }
        format!("{message} This org has {}.", env_policy::NO_BRANCH_NOTE)
    }

    /// The DSN an OLTP connection opens with. Outside production, held, the
    /// session defaults every transaction to read-only.
    pub(super) fn oltp_dsn(&self, op: HostOp, dsn: &str) -> String {
        if self.holds(op) {
            read_only_session(dsn)
        } else {
            dsn.to_string()
        }
    }

    /// Put an OLTP transaction in `READ ONLY` mode when `op` is held, and log
    /// that it was opened so. Postgres then refuses a write the classifier
    /// cannot see — one inside a function the app defined.
    pub(super) async fn read_only_when_held(
        &self,
        op: HostOp,
        tx: &mut dyn SqlTransaction,
        namespace: &str,
    ) -> Result<(), String> {
        if !self.holds(op) {
            return Ok(());
        }
        with_db_timeout("read-only", async {
            tx.exec("SET TRANSACTION READ ONLY", &[])
                .await
                .map(|_| ())
                .map_err(|e| format!("could not open a read-only transaction: {e}"))
        })
        .await?;
        if op == HostOp::TxBeginOltp {
            self.note_held_oltp(op, ("oltp", namespace, "BEGIN READ ONLY", ""))
                .await;
        }
        Ok(())
    }

    /// Turn Postgres's read-only refusal of a held op into the held message,
    /// and log it, so the write reports why it did not happen.
    pub(super) async fn held_oltp_error(
        &self,
        op: HostOp,
        err: String,
        namespace: &str,
        summary: &QuerySummary,
    ) -> String {
        if !self.holds(op) || !env_policy::is_read_only_violation(&err) {
            return err;
        }
        self.note_held_oltp(op, ("oltp", namespace, &summary.verb, &summary.table))
            .await;
        self.with_branch_note(self.held_error(op))
    }
}

/// `dsn` with [`READ_ONLY_SESSION`] added to its `options` parameter — the one
/// that already carries the writer's `search_path` — or as a new one.
fn read_only_session(dsn: &str) -> String {
    let Some((base, query)) = dsn.split_once('?') else {
        return format!("{dsn}?options={READ_ONLY_SESSION}");
    };
    let mut found = false;
    let params: Vec<String> = query
        .split('&')
        .map(|param| match param.strip_prefix("options=") {
            Some(value) => {
                found = true;
                format!("options={value}%20{READ_ONLY_SESSION}")
            }
            None => param.to_string(),
        })
        .collect();
    let mut query = params.join("&");
    if !found {
        // A DSN ending in a bare `?` has no parameter to join onto.
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(&format!("options={READ_ONLY_SESSION}"));
    }
    format!("{base}?{query}")
}

#[cfg(test)]
mod tests {
    use super::{
        CHANGES_SCHEMA_REFUSAL, PLATFORM_SCHEMA_REFUSAL, read_only_session, refuse_platform_schema,
    };

    #[test]
    fn sql_naming_the_platform_events_schema_is_refused_however_spelled() {
        for sql in [
            "select oxy_events.publish($1)",
            "DELETE FROM OXY_EVENTS.outbox",
            "select * from \"oxy_events\".outbox",
            "do $$ begin execute 'select 1 from oxy_events.outbox'; end $$",
            "select 'Oxy_Events.outbox'::regclass",
            "grant usage on schema oxy_events to public",
            "SET search_path TO oxy_events",
            "set local search_path = 'oxy_events, public'",
        ] {
            assert_eq!(
                refuse_platform_schema(sql),
                Err(PLATFORM_SCHEMA_REFUSAL.to_string()),
                "{sql}"
            );
        }
    }

    /// The change stream's schema, its log and the settings the host signs a
    /// production transaction with, refused as `oxy_events` is, each with its
    /// own message; names that merely resemble it are sent.
    #[test]
    fn oltp_sql_naming_oxy_changes_is_refused() {
        for sql in [
            "select * from oxy_changes.log",
            "DELETE FROM OXY_CHANGES.log",
            "select * from \"oxy_changes\".tables",
            "truncate oxy_changes.log",
            "do $$ begin execute 'delete from oxy_changes.log'; end $$",
            "update oxy_changes.stream set capturing = false",
            "select set_config('oxy_changes.actor', 'someone', true)",
            "select pg_catalog.set_config('oxy_changes.provenance', $1, true)",
            "SET LOCAL oxy_changes.invocation = '00000000-0000-0000-0000-000000000000'",
            "select current_setting('oxy_changes.provenance', true)",
            "SET search_path TO oxy_changes",
            // Unreadable by the tokenizer (an unterminated string): searched
            // as text.
            "select 'oxy_changes.log",
        ] {
            assert_eq!(
                refuse_platform_schema(sql),
                Err(CHANGES_SCHEMA_REFUSAL.to_string()),
                "{sql}"
            );
        }
        assert!(CHANGES_SCHEMA_REFUSAL.starts_with("OltpPlatformSchema: `oxy_changes` "));
        for sql in [
            "select * from my_oxy_changes",
            "select oxy_changes_count from t",
            "select * from \"OXY_CHANGES\".log",
            "select 1 -- oxy_changes.log is the platform's\n",
            "insert into notes (body) values ('oxy_changesx')",
        ] {
            assert_eq!(refuse_platform_schema(sql), Ok(()), "{sql}");
        }
    }

    /// The review's spellings, which name the schema only once Postgres reads
    /// them: a Unicode-escaped identifier, and `search_path` set to a value
    /// built when the statement runs (then an unqualified `publish(...)`).
    #[test]
    fn sql_that_spells_the_schema_only_when_it_runs_is_refused() {
        for sql in [
            r#"select U&"oxy\005fevents".publish($1)"#,
            r#"select u&"oxy!005fevents" uescape '!' .outbox"#,
            r#"select U&"set_confi\0067"('search_path', 'public', true)"#,
            "select set_config('search_path', 'oxy' || '_events', true)",
            "select pg_catalog.set_config('search_path', $1, true)",
            "select SET_CONFIG ( 'search_path' , format('%s_events', 'oxy'), false )",
            "select set_config('SEARCH_PATH', E'oxy\\137events', true)",
            "select set_config('search_path', 'oxy\\137events', true)",
            "select set_config(name, value, true) from (values ('search_path', 'x')) v(name, value)",
            "select set_config(E'search\\137path', 'public', true)",
            "select \"set_config\"(setting_name => 'search_path', new_value => 'public', is_local => true)",
            "SET search_path = E'oxy\\137events'",
            "set local search_path to U&'oxy\\005fevents'",
            "alter role current_user set search_path = $$oxy_e$$",
            "update pg_settings set setting = 'oxy' || '_events' where name = 'search_path'",
            "UPDATE pg_catalog.PG_SETTINGS SET setting = $1 WHERE name = $2",
        ] {
            let refused = refuse_platform_schema(sql).expect_err(sql);
            assert!(
                refused.starts_with("OltpPlatformSchema: "),
                "{sql}: {refused}"
            );
        }
    }

    #[test]
    fn sql_merely_resembling_it_is_sent() {
        for sql in [
            "select * from events",
            "select * from my_oxy_events",
            "select oxy_events_archive from t",
            "insert into notes (body) values ('oxy_eventsx')",
            "select 1 -- oxy_events.outbox is the platform's\n",
            "select 1 /* oxy_events */",
            "select * from \"OXY_EVENTS\".outbox",
            "select set_config('app.tenant', $1, true)",
            "select set_config('default_transaction_read_only', 'off', false)",
            "select set_config('search_path', 'public, app_x', true)",
            "set search_path = public",
            "SET LOCAL search_path TO app_x, public",
            "show search_path",
            "select current_setting('search_path')",
            "select name, setting from pg_settings where name = 'search_path'",
            "select u & flags from t",
            "select E'line\\n' as text",
        ] {
            assert_eq!(refuse_platform_schema(sql), Ok(()), "{sql}");
        }
    }

    /// What the guard cannot see: SQL built while it runs, by a `DO` block or
    /// a function the app defined. The writer may call `oxy_events.publish`
    /// that way; the definer function's bounds (platform step 5) and the relay
    /// are what hold there. Pinned so a change to that is deliberate.
    #[test]
    fn sql_built_when_it_runs_is_not_seen() {
        for sql in [
            "do $$ begin execute 'select 1 from oxy' || '_events.outbox'; end $$",
            "select app_run_text('select 1 from oxy' || '_events.outbox')",
        ] {
            assert_eq!(refuse_platform_schema(sql), Ok(()), "{sql}");
        }
    }

    #[test]
    fn the_read_only_default_joins_the_writers_search_path_option() {
        assert_eq!(
            read_only_session(
                "postgresql://w:p@h:5432/db?sslmode=require&options=-csearch_path%3Dapp_x"
            ),
            "postgresql://w:p@h:5432/db?sslmode=require&options=-csearch_path%3Dapp_x\
             %20-cdefault_transaction_read_only%3Don"
        );
        assert_eq!(
            read_only_session("postgresql://w:p@h/db?sslmode=require"),
            "postgresql://w:p@h/db?sslmode=require&options=-cdefault_transaction_read_only%3Don"
        );
        assert_eq!(
            read_only_session("postgresql://w:p@h/db"),
            "postgresql://w:p@h/db?options=-cdefault_transaction_read_only%3Don"
        );
        assert_eq!(
            read_only_session("postgresql://w:p@h/db?"),
            "postgresql://w:p@h/db?options=-cdefault_transaction_read_only%3Don"
        );
    }

    /// The session option survives the DSN parser the connector uses.
    #[test]
    fn the_connector_reads_both_options() {
        let dsn = read_only_session("postgresql://w:p@h/db?options=-csearch_path%3Dapp_x");
        let config: tokio_postgres::Config = dsn.parse().expect("parses");
        assert_eq!(
            config.get_options(),
            Some("-csearch_path=app_x -cdefault_transaction_read_only=on")
        );
    }
}
