//! What a failed invocation says on the platform's side.
//!
//! In 0.5.140–0.5.144 every `ctx.warehouse.insert` against ClickHouse failed,
//! and every failure was recorded — in `app_function_invocations.error` and the
//! tenant-facing ClickHouse sink. Neither is a place anyone on call looks. No
//! platform log line named the failure, the invocation span was never marked
//! failed, and nothing paged; a customer reported it, and a first-pass
//! investigation that searched the platform logs found nothing and guessed.
//!
//! This is the shape of a failure the platform can hold without holding its
//! message: a coarse [`Failure::kind`] (a HyperDX facet, a Slack line) and a
//! [`fingerprint`] that names "the same failure" across invocations and apps.
//! The message itself stays in the tenant's store, behind the app-admin gate —
//! the same rule `host_call_attrs` keeps for host-op spans. A fingerprint is
//! pseudonymous rather than anonymous: it hides the message, but someone who
//! already holds a message and a guessed value can check the guess against it.

use sha2::{Digest, Sha256};

/// How an invocation failed, with nothing of what it said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Failure {
    /// `threw` (the app's code threw) | `internal` (the runtime failed) |
    /// `exceeded_memory` (the app breached its isolate's heap ceiling) |
    /// `platform` (the invocation never reached the app's code) | `timeout` |
    /// `http_5xx` | `host_call` (the app answered below 500 after a `ctx.*`
    /// call failed). Bounded on purpose: a facet with one value per message is
    /// no facet.
    pub kind: &'static str,
    /// See [`fingerprint`].
    pub fingerprint: String,
    /// The call behind a `host_call` failure, by op and kind; `None` for every
    /// other kind. Carried because that row's `error` is NULL — the app caught
    /// the message — so these two closed-list values are what the page and the
    /// log line can name.
    pub host_call: Option<HostCallFailure>,
}

/// The first `ctx.*` call of an invocation that failed on the platform's side:
/// its name, and the shape of what the host said. `op` is a fixed op name
/// (`query`, `warehouse.insert`), never the SQL, URL or secret name the call
/// carried; `kind` is a `host_call_attrs::classify_host_error` value that
/// counts toward paging; `message` is the host's error text after
/// [`normalize`] and the [`FINGERPRINT_PREFIX`] bound — the same text the
/// `threw` path digests — taken as the failure is noted, so the raw text is
/// never held. It feeds the fingerprint and nothing else: not the span, the
/// log line or the page, which name the op and kind. `target` is where the
/// call went, for the ops that name one — a `fetch`'s host, a `warehouse.*`
/// op's database and table — normalized the same way and kept so that a
/// later success of the op clears the note only when it went to the same
/// place ([`HostCallFailure::recovered`]). It joins the fingerprint only for
/// a `fetch` whose message does not name its host
/// ([`HostCallFailure::fingerprint`]).
//
// The message is here because op and kind alone masked a new failure: a
// function that already catches a routine `permission_denied` on
// `warehouse.insert` made a real break of the same op and kind "not new" to
// the pager, so it never paged.
//
// The target is here because a clear by op alone masked the other half: the
// fingerprint kept a URL's host so two vendors under one `fetch` were two
// patterns, and then a caught failure on vendor A was dropped by any later
// `fetch` that worked — a Slack webhook, say — so A's break never paged. It
// then reached the fingerprint because the host was kept only where the
// message carried the URL: a body read that timed out names no host, so two
// vendors' timeouts were one pattern after all.
//
// Only the V8 runtime constructs one; without it the type is still named by
// the run outcome, which always carries `None`.
//
// `pub`, not `pub(super)`: `FunctionHost::host_call_failure` is a method of a
// `pub` trait, and a `pub(super)` return type there trips `private_interfaces`.
// This module is private, so the type still reaches no further than
// `custom_apps_functions`.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
#[derive(Clone, PartialEq, Eq)]
pub struct HostCallFailure {
    pub op: &'static str,
    pub kind: &'static str,
    pub message: String,
    /// Where the call went, normalized as `message` is; `None` for an op
    /// that names no target, which then clears by op alone.
    pub target: Option<String>,
}

/// Written by hand so that `message` cannot reach a log line through a
/// `?failure` field or a panic message: the op and kind print, and the message
/// prints as its digest — enough to tell two values apart in a failed
/// assertion, with none of the text. [`Failure`] derives its `Debug` and holds
/// one of these, so the rule covers it too.
impl std::fmt::Debug for HostCallFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostCallFailure")
            .field("op", &self.op)
            .field("kind", &self.kind)
            .field(
                "message",
                &format_args!("<elided, digest {}>", digest(&self.message)),
            )
            // Shape, not payload: a host or a database name, already on the
            // call's own span (`server.address`, `db.namespace`).
            .field("target", &self.target)
            .finish()
    }
}

/// The rule for the note a host keeps across one invocation's calls. The host
/// holds the state (`ProjectFunctionHost::host_call_failure`); the broker
/// reports each call's outcome; these decide what the note becomes.
#[cfg_attr(not(feature = "custom-app-functions"), allow(dead_code))]
impl HostCallFailure {
    /// After a call of `op` against `target` failed as `kind`, saying
    /// `message`: the first failure stands. One page names one failure, and
    /// later calls often fail because of it. The message and the target are
    /// normalized here, before anything is kept — the host holds the note for
    /// the rest of the run, and what it holds must already be the digest's
    /// input, not the data.
    pub fn noted(
        current: Option<Self>,
        op: &'static str,
        kind: &'static str,
        message: &str,
        target: Option<&str>,
    ) -> Option<Self> {
        current.or_else(|| {
            Some(Self {
                op,
                kind,
                message: normalized_prefix(message),
                target: target.map(normalized_prefix),
            })
        })
    }

    /// After a call of `op` against `target` succeeded: a failure of that op
    /// against that target is one the run recovered from (a retried
    /// `ctx.fetch` that answered on the second attempt), and the fingerprint
    /// should name a failure it did not. The target is part of the key: a
    /// `fetch` to vendor A that failed is not answered for by a `fetch` to
    /// Slack that worked, and clearing it there erased the one failure the
    /// pager exists to see. The target is compared normalized, as it was
    /// kept, so the digit rule folds on both sides: a numbered shard is one
    /// host (`shop123.example.com` and `shop456.example.com`), and a
    /// numbered table is one table the same way — `dw orders_2024` and
    /// `dw raw_v1` are both `dw #`, so a caught insert into one is cleared by
    /// a success on the other; only digit-free names tell apart. An op that
    /// names no target (`None` on both sides) clears by op name alone. The
    /// raw target goes no further than this call.
    pub fn recovered(
        current: Option<Self>,
        op: &'static str,
        target: Option<&str>,
    ) -> Option<Self> {
        current.filter(|hc| !(hc.op == op && hc.target == target.map(normalized_prefix)))
    }

    /// The fingerprint a caught failure pages under: a digest over the op,
    /// the kind and the normalized message — and, for a `fetch` whose message
    /// does not name its host, the target. Op and kind keep one broken call
    /// together across every app it hit; the message keeps a new break apart
    /// from the routine failure a function already catches on the same op, so
    /// it is still new to the pager. Digested, none of the message's text
    /// reaches the row, the log line or the page.
    ///
    /// The target rule, exactly: the target joins the input when the op is
    /// on [`TARGET_KEYED_OPS`] and the normalized message does not contain
    /// `<target>/?` — the one shape [`url_host`] writes a host in, so a
    /// coincidental substring (a single-label host that folded to `#`, found
    /// in `# bytes`) does not count as naming it. A `fetch` is keyed by host
    /// wherever its message carries the URL bare (`url_host` keeps
    /// `api.example.com` in `for url (api.example.com/?)`, which is where
    /// reqwest puts every send-phase error, a timeout included), so those
    /// messages already carry the target and keep the fingerprint they have.
    /// The ones that name no host
    /// gain it: a body read that failed or timed out (`read failed: error
    /// decoding response body` — reqwest's `chunk()` carries no URL), a
    /// response over the size cap, and the refusal that quotes its URL as a
    /// value (`fetch to ? blocked …`). Without it, a routine one of those on
    /// vendor A made a new one on vendor B "not new" to the pager. ` @ `
    /// separates the two, and cannot occur in the message: a run with `@` in
    /// it is a locator and folds to `?`, so no normalized text contains one.
    ///
    /// Every fingerprint this changes pages once more on deploy day, which is
    /// why the rule reaches no further. A `warehouse.*` target stays out: no
    /// warehouse message names its database, so folding it in would
    /// re-fingerprint every caught warehouse failure at once, for a
    /// per-destination split the fingerprint never had.
    pub fn fingerprint(&self) -> String {
        let mut input = format!("host_call {} {} {}", self.op, self.kind, self.message);
        if let Some(target) = self.unnamed_target() {
            input.push_str(" @ ");
            input.push_str(target);
        }
        digest(&input)
    }

    /// The target the fingerprint carries: the note's, when the op is keyed
    /// by it and the message does not already name it as `url_host` writes
    /// a host — `<host>/?`. Both sides are normalized, so a numbered shard's
    /// host is found in a message that carries it (`#.example.com` in
    /// `(#.example.com/?)`).
    fn unnamed_target(&self) -> Option<&str> {
        let target = self.target.as_deref()?;
        let named = self.message.contains(&format!("{target}/?"));
        (TARGET_KEYED_OPS.contains(&self.op) && !named).then_some(target)
    }
}

/// The ops whose target joins the fingerprint when the message does not name
/// it ([`HostCallFailure::fingerprint`]). `fetch` alone: its target is the
/// host the fingerprint already keys on wherever the message carries the URL,
/// so this is the same rule reaching the messages that carry none. Adding an
/// op here re-fingerprints every caught failure of that op whose message
/// does not name the target — for `warehouse.*`, all of them.
const TARGET_KEYED_OPS: &[&str] = &["fetch"];

impl Failure {
    /// The failure an invocation's outcome describes, or `None` when it did not
    /// fail. A cancellation is someone asking it to stop, not the app failing.
    /// A handler that caught an error and answered 5xx did fail — that is how a
    /// broken host call looks from an app that handles its errors. So did one
    /// that answered below 500 after a host call failed (`host_call`): catching
    /// the error hides it from the response, not from on-call.
    pub fn of(
        status: &str,
        http_status: u16,
        error: Option<&str>,
        host_call: Option<&HostCallFailure>,
    ) -> Option<Self> {
        let (kind, fingerprint) = match status {
            "cancelled" => return None,
            // A shed is the platform declining to start work it has no capacity
            // for — the concurrency cap doing its job, not a failure of
            // anything. Paging per shed would be a storm precisely when the
            // fleet is busiest, and charging it to the app would make a
            // capacity decision look like the tenant's bug.
            // `oxy_custom_app_admission_shed_total` is where sheds are counted,
            // and it carries which limit bound.
            "shed" => return None,
            "success" if http_status < 500 => {
                return host_call.map(|hc| Self {
                    kind: "host_call",
                    fingerprint: hc.fingerprint(),
                    host_call: Some(hc.clone()),
                });
            }
            // Digested whole, not normalized: the status IS the signature, and
            // normalizing would fold a new 500 into a function's usual 503.
            "success" => ("http_5xx", digest(&format!("http status {http_status}"))),
            "timeout" => ("timeout", Self::timeout_fingerprint()),
            _ => {
                let message = error.unwrap_or_default();
                (kind_of_error(message), fingerprint(message))
            }
        };
        Some(Self {
            kind,
            fingerprint,
            host_call: None,
        })
    }

    /// The fingerprint every timeout shares, for the reaper, which marks
    /// orphaned rows `timeout` without running [`Failure::of`] per row.
    pub fn timeout_fingerprint() -> String {
        digest("timeout")
    }

    /// Mark the enclosing invocation span failed and put one WARN line on the
    /// platform log. Call inside the `custom_app_function` span: the line then
    /// carries that span's app, function, invocation and request ids, which is
    /// how on-call gets from HyperDX to the invocation row and its message.
    #[cfg(feature = "custom-app-functions")]
    pub fn report(&self) {
        let span = tracing::Span::current();
        span.record("otel.status_code", "ERROR");
        span.record("error.type", self.kind);
        // A `host_call` failure's message went to the app's catch block, not
        // the invocation row, so the op and kind are what on-call has to go on.
        let (op, host_kind) = self
            .host_call
            .as_ref()
            .map_or((None, None), |hc| (Some(hc.op), Some(hc.kind)));
        span.record("host_call.op", op);
        span.record("host_call.kind", host_kind);
        // Braced: `type` is a keyword, so the field is a string literal, and
        // unbraced the macro cannot tell a literal field from the message.
        tracing::warn!(
            target: "oxy::app_function",
            {
                error.fingerprint = %self.fingerprint,
                "error.type" = %self.kind,
                host_call.op = op,
                host_call.kind = host_kind
            },
            "custom-app function invocation failed"
        );
    }
}

/// Who an `error` status belongs to, from the prefix `RuntimeError` puts on the
/// message. Anything without one failed before the isolate ran — a workspace
/// or build lookup, a missing function — which is the platform's, not the app's.
fn kind_of_error(message: &str) -> &'static str {
    if message.starts_with("function threw:") {
        "threw"
    } else if message.starts_with("internal runtime error") {
        "internal"
    } else if message.starts_with("function exceeded its memory limit") {
        // The app's fault, and the fallback below would have called it the
        // platform's — inverting the attribution on exactly the failure the
        // heap ceiling exists to make attributable. Its own value rather than
        // folding into `threw`, because the remedy differs: `threw` is a bug in
        // the handler, this is the app holding more live objects than an
        // isolate may.
        "exceeded_memory"
    } else {
        "platform"
    }
}

/// Chars of normalized message that feed the digest — chars, not bytes, so
/// the bound never lands inside a multi-byte character. Past this, messages
/// that share a prefix are the same failure; stack traces and quoted rows vary
/// further down without saying anything new.
const FINGERPRINT_PREFIX: usize = 240;

/// A stable name for "the same failure": 16 hex chars of a digest over the
/// message with its data taken out.
///
/// Two invocations that failed the same way differ in everything the message
/// says about the data — quoted values, row numbers, ids, the rows ClickHouse
/// quotes back, the object key or URL a store or fetch names. [`normalize`]
/// takes those out before hashing, so the Code 27 insert failure is one
/// fingerprint across every invocation and every app it hit.
pub(super) fn fingerprint(message: &str) -> String {
    digest(&normalized_prefix(message))
}

/// The first [`FINGERPRINT_PREFIX`] chars of the normalized message: the
/// digest's input, and all a [`HostCallFailure`] keeps of what the host said.
fn normalized_prefix(message: &str) -> String {
    let mut normalized = normalize(message);
    if let Some((end, _)) = normalized.char_indices().nth(FINGERPRINT_PREFIX) {
        normalized.truncate(end);
    }
    normalized
}

fn digest(input: &str) -> String {
    hex::encode(&Sha256::digest(input.as_bytes())[..8])
}

/// The message with its data taken out, three rules:
///
/// - a quoted run becomes `?` — the value a warehouse or the host quotes
///   back (`Cannot parse input: expected '(' before: '…'`, `fetch to '…'
///   blocked`);
/// - a run containing `/` or `@` becomes `?` — a URL, path, object key or
///   address the message carries bare: reqwest's `error sending request for
///   url (https://…)`, the stores' `head_object <key>: …` and `write <path>:
///   …`, a mail address in an SES refusal. Each embeds a value that changes
///   per call (a path segment, a file name), which the digit rule alone does
///   not catch, so without this every occurrence was a new fingerprint and
///   none reached the paging threshold. A URL keeps its host in front of the
///   `?` ([`url_host`]): `https://api.example.com:8443/v1/x?page=abc` is
///   `api.example.com/?`. Everything with no host folds whole;
/// - every other word containing a digit becomes `#` — a row number, an id,
///   a version, a size.
///
/// A run is what lies between spaces and brackets (`()[]{}`, `,`, `;`). A
/// colon inside one does not end it — a URL's scheme and port are read with
/// the rest of it, so one host is one input with or without a port — but a
/// colon or full stop ending it stays outside the `?`, as do a URL's
/// parentheses: `head_object ?: …`, `for url (api.example.com/?)`.
/// `DB::Exception:` reads as before. Whitespace collapses.
fn normalize(message: &str) -> String {
    let mut out = String::with_capacity(message.len().min(FINGERPRINT_PREFIX * 2));
    let mut chars = message.chars().peekable();
    let mut run = String::new();
    let mut prev = ' ';
    while let Some(c) = chars.next() {
        match c {
            // A quote opens a quoted run only where a value could start: after
            // a word character it is an apostrophe (`doesn't`, `app's`), and
            // treating it as a quote would swallow the rest of the message.
            '\'' | '"' | '`' if !is_word_char(prev) => {
                flush_run(&mut run, &mut out);
                skip_quoted(c, &mut chars);
                out.push('?');
            }
            c if c.is_whitespace() => {
                flush_run(&mut run, &mut out);
                if !out.ends_with(' ') {
                    out.push(' ');
                }
            }
            c if is_run_boundary(c) => {
                flush_run(&mut run, &mut out);
                out.push(c);
            }
            c => run.push(c),
        }
        prev = c;
    }
    flush_run(&mut run, &mut out);
    out.trim().to_string()
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn is_run_boundary(c: char) -> bool {
    matches!(c, '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';')
}

/// Emit one run. A locator — a run with `/` or `@` in it — becomes `?`, after
/// its host when it is a URL ([`url_host`]), and with the colon or full stop
/// that ends it kept, since that is the sentence's and not the locator's. Any
/// other run emits its words with the digit rule applied and its punctuation
/// kept.
fn flush_run(run: &mut String, out: &mut String) {
    if run.is_empty() {
        return;
    }
    let locator_len = run.trim_end_matches([':', '.']).len();
    let locator = &run[..locator_len];
    if locator.contains(['/', '@']) {
        if let Some(host) = url_host(locator) {
            push_words(&host, out);
            out.push('/');
        }
        out.push('?');
        out.push_str(&run[locator_len..]);
    } else {
        push_words(run, out);
    }
    run.clear();
}

/// The host of a locator that is a URL — `scheme://authority…` with a real
/// scheme and a host — lowercased, without userinfo or port. `None` for
/// everything else a locator can be: an object key, a filesystem path, a mail
/// address, a `file:///…` module specifier (no authority), a path whose query
/// string happens to carry a URL (no scheme in front).
///
/// The host is kept because the pager keys on (app, function, fingerprint).
/// Folded whole, every URL one function fetches was one input, so a routine
/// failure on one vendor made a new break on another "not new" — the mask
/// this fingerprint exists to remove, one level down. Scheme, port, path and
/// query are what change per call or say nothing, so they still fold. A host
/// is shape, not payload, by the rule that already puts it on a platform span;
/// the parser is the one that span uses (`url_shape::fetch_target`), so the
/// two cannot disagree. It is read from `url_shape` and not from
/// `host_call_attrs`, which is gated with the runtime: this module compiles,
/// and must normalize identically, with the feature off.
/// The host then takes the digit rule like any other words: a numbered shard
/// (`api2.…`, `shop123.…`) is one input, not one per number.
fn url_host(locator: &str) -> Option<String> {
    let target = super::url_shape::fetch_target(locator);
    let is_scheme = target.scheme.starts_with(|c: char| c.is_ascii_alphabetic())
        && target
            .scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    let is_host = !target.host.is_empty()
        && target
            .host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
    (is_scheme && is_host).then_some(target.host)
}

/// `text` with every word containing a digit as `#`, punctuation kept.
fn push_words(text: &str, out: &mut String) {
    let mut word = String::new();
    for c in text.chars() {
        if is_word_char(c) {
            word.push(c);
        } else {
            flush_word(&mut word, out);
            out.push(c);
        }
    }
    flush_word(&mut word, out);
}

fn flush_word(word: &mut String, out: &mut String) {
    if word.chars().any(|c| c.is_ascii_digit()) {
        out.push('#');
    } else {
        out.push_str(word);
    }
    word.clear();
}

/// Consume a quoted run up to its closing `quote`, honouring `\`-escapes and a
/// doubled quote. An unterminated run swallows the rest, which is still one `?`.
fn skip_quoted(quote: char, chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = chars.next() {
        if c == '\\' {
            chars.next();
        } else if c == quote {
            if chars.peek() == Some(&quote) {
                chars.next();
            } else {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The error every ClickHouse insert returned in 0.5.140–0.5.144, as two
    /// different invocations of two different apps saw it.
    const CODE_27_A: &str = "function threw: Error: warehouse insert failed: query failed: HTTP 400 \
        Bad Request: Code: 27. DB::Exception: Cannot parse input: expected '(' before: \
        '/*oxy.app=\\'bookkeeping\\',oxy.fn=\\'upload-report\\',oxy.invocation=\\'0af7651916cd\\'*/': \
        at row 63: While executing ValuesBlockInputFormat. (CANNOT_PARSE_INPUT_ASSERTION_FAILED) \
        (version 25.8.4.13 (official build))";
    const CODE_27_B: &str = "function threw: Error: warehouse insert failed: query failed: HTTP 400 \
        Bad Request: Code: 27. DB::Exception: Cannot parse input: expected '(' before: \
        '/*oxy.app=\\'receiving\\',oxy.fn=\\'submit\\',oxy.invocation=\\'884e14953bc3\\'*/': \
        at row 2: While executing ValuesBlockInputFormat. (CANNOT_PARSE_INPUT_ASSERTION_FAILED) \
        (version 25.8.4.13 (official build))";

    #[test]
    fn the_same_failure_in_different_apps_and_rows_is_one_fingerprint() {
        assert_eq!(fingerprint(CODE_27_A), fingerprint(CODE_27_B));
        assert_eq!(fingerprint(CODE_27_A).len(), 16);
        // The value `main` gave this message before `normalize` learned about
        // locators, computed from a separate implementation of that older
        // algorithm. A message with no bare `/` or `@` run keeps the
        // fingerprint it has, so rows already stored stay in their groups and
        // the locator rule re-pages only the failures it re-reads.
        assert_eq!(fingerprint(CODE_27_A), "af597dfd3a7536b8");
    }

    /// What bounds the locator rule's reach into failures that were never
    /// caught. A thrown error is stored with its stack (`JsError`'s `Display`
    /// prints it), so a frame sits inside nearly every short message's prefix
    /// — but this runtime names its modules `oxy:function`, `oxy:bootstrap`
    /// and `oxy:invoke`, with no `/`, so a function's own frames are not
    /// locators and an ordinary thrown error keeps the fingerprint `main` gave
    /// it (the literal, from the same separate implementation as Code 27's).
    /// Rename those specifiers to `file:///…` and every `threw` fingerprint on
    /// the platform changes at once: this test is what says so.
    #[test]
    fn a_thrown_errors_own_stack_frames_keep_the_fingerprint_main_gave_it() {
        let thrown = "function threw: Error: name is required\n    \
                      at default (oxy:function:3:9)\n    at async oxy:invoke:5:20";
        assert_eq!(
            normalize(thrown),
            "function threw: Error: name is required at default (oxy:function:#:#) \
             at async oxy:invoke:#:#"
        );
        assert_eq!(fingerprint(thrown), "d5267ee67e7f0e3a");
        // A frame that does carry a slash is a locator, and folds: a thrown
        // error whose prefix reaches one is re-fingerprinted by this rule.
        assert_eq!(
            normalize("at eventLoopTick (ext:core/01_core.js:178:7)"),
            "at eventLoopTick (?)"
        );
    }

    #[test]
    fn a_different_failure_is_a_different_fingerprint() {
        let missing_table = "function threw: Error: warehouse insert failed: query failed: HTTP 404 \
            Not Found: Code: 60. DB::Exception: Table default.receiving does not exist. \
            (UNKNOWN_TABLE) (version 25.8.4.13 (official build))";
        assert_ne!(fingerprint(CODE_27_A), fingerprint(missing_table));
        assert_ne!(
            fingerprint("function threw: Error: name is required"),
            fingerprint("function threw: Error: partySize is required")
        );
    }

    #[test]
    fn the_data_in_a_message_does_not_reach_the_fingerprint_input() {
        let normalized = normalize(CODE_27_A);
        for leaked in ["bookkeeping", "upload-report", "0af7651916cd", "63"] {
            assert!(!normalized.contains(leaked), "{leaked} in {normalized}");
        }
        assert_eq!(
            normalize("row 'it''s' at \"col\" id 4bf92f35 uuid 0b9f4c7e-1d2a"),
            "row ? at ? id # uuid #-#"
        );
        // An apostrophe is not a quote: the words after it still count.
        assert_ne!(
            fingerprint("Table t doesn't exist. (UNKNOWN_TABLE)"),
            fingerprint("Table t doesn't exist. (UNKNOWN_DATABASE)")
        );
        assert_eq!(
            normalize("could not connect to this app's Airhouse schema: refused"),
            "could not connect to this app's Airhouse schema: refused"
        );
    }

    /// One message per shape the host writes bare — a URL, an object key, a
    /// path, an address — each as `host.rs`, reqwest or the asset stores
    /// phrase it, with the value that changes per call. Each shape folds to
    /// one input, so the occurrences count as one failure rather than each
    /// being new. Dropping the locator rule from `normalize` fails its line.
    #[test]
    fn a_locator_the_host_writes_bare_does_not_reach_the_fingerprint_input() {
        // reqwest 0.13 appends ` for url (<url>)` to `fetch failed: {e}`. The
        // host stays (see the two-endpoints test); what changes per call goes.
        let fetch_a = "fetch failed: error sending request for url \
                       (https://api.example.com/v1/customers/acme-corp/invoices)";
        let fetch_b = "fetch failed: error sending request for url \
                       (https://api.example.com:8443/v1/customers/globex/invoices?page=abc)";
        assert_eq!(
            normalize(fetch_a),
            "fetch failed: error sending request for url (api.example.com/?)"
        );
        assert_eq!(normalize(fetch_a), normalize(fetch_b));
        // `s3::head` / `s3::copy`: the key bare, then the SDK's error.
        let head_a = "s3 error: head_object customer-app-storage/3fa85f64-5717-4562-b3fc-2c963f66afa6/\
                      uploads/invoice-acme.pdf: dispatch failure";
        let head_b = "s3 error: head_object customer-app-storage/3fa85f64-5717-4562-b3fc-2c963f66afa6/\
                      uploads/statement-globex.pdf: dispatch failure";
        // (`s3` carries a digit, so the digit rule already reads it as `#`.)
        assert_eq!(
            normalize(head_a),
            "# error: head_object ?: dispatch failure"
        );
        assert_eq!(normalize(head_a), normalize(head_b));
        assert_eq!(
            normalize(
                "s3 error: copy_object customer-app-storage/a/reports/q3.csv -> \
                 customer-app-storage/a/archive/q3.csv: service error"
            ),
            "# error: copy_object ? -> ?: service error"
        );
        // The filesystem store: `write <path>: <io error>`.
        assert_eq!(
            normalize(
                "filesystem storage error: write /var/oxy/state/customer-app-storage/a/uploads/\
                 invoice-acme.pdf: No such file or directory (os error 2)"
            ),
            "filesystem storage error: write ?: No such file or directory (os error #)"
        );
        // SES names the recipient it refused.
        assert_eq!(
            normalize(
                "MessageRejected: Email address is not verified. The following identities \
                 failed the check in region US-EAST-1: bob@example.com"
            ),
            "MessageRejected: Email address is not verified. The following identities \
             failed the check in region US-EAST-#: ?"
        );
        for (message, leaked) in [
            (fetch_a, "acme-corp"),
            (fetch_a, "https"),
            (fetch_b, "globex"),
            (fetch_b, "8443"),
            (fetch_b, "page"),
            (head_a, "invoice-acme"),
            (head_a, "customer-app-storage"),
        ] {
            assert!(
                !normalize(message).contains(leaked),
                "{leaked} in {}",
                normalize(message)
            );
        }
        // The host's own refusals already quote the value; they read as before.
        assert_eq!(
            normalize("fetch to 'http://rates.example.com/v1/usd' blocked by SSRF allowlist"),
            "fetch to ? blocked by SSRF allowlist",
            "custom_app_functions_host_failures computes its fingerprint over this text"
        );
        assert_eq!(
            normalize("'metadata.internal' resolves only to non-public addresses"),
            "? resolves only to non-public addresses"
        );
    }

    /// The mask, one level down. The pager keys on (app, function,
    /// fingerprint), so with every URL folded to one `?` a function that
    /// already catches a routine timeout on one vendor had a new break on
    /// another arrive as "not new". A URL keeps its host; what changes per
    /// call — scheme, port, path, query, userinfo — still folds, so one host
    /// is one input. Collapsing the host again in `flush_run` fails this.
    #[test]
    fn two_endpoints_in_one_function_are_two_fingerprints() {
        let fetch_failed = |url: &str| {
            HostCallFailure::noted(
                None,
                "fetch",
                "host_call_failed",
                &format!("fetch failed: error sending request for url ({url})"),
                None,
            )
            .unwrap()
        };
        let vendor_a = fetch_failed("https://api.example.com/v1/rates");
        let vendor_b = fetch_failed("https://api.other.com/z");
        assert_eq!(
            vendor_a.message,
            "fetch failed: error sending request for url (api.example.com/?)"
        );
        assert_eq!(
            vendor_b.message,
            "fetch failed: error sending request for url (api.other.com/?)"
        );
        assert_ne!(
            vendor_a.fingerprint(),
            vendor_b.fingerprint(),
            "a break on one vendor is new beside a routine failure on another"
        );

        // One host is one input, however the URL varies from call to call.
        for url in [
            "https://api.example.com",
            "https://api.example.com/",
            "https://api.example.com:8443/v1/x?page=abc",
            "https://api.example.com/v1/y",
            "http://api.example.com/v1/customers/acme-corp#frag",
            "https://user:hunter2@api.example.com/v1/rates?key=sk_live_abc",
            "HTTPS://API.Example.COM/v1/rates",
        ] {
            assert_eq!(
                fetch_failed(url).fingerprint(),
                vendor_a.fingerprint(),
                "{url}"
            );
        }
        for kept_out in ["hunter2", "sk_live_abc", "user", "8443", "acme-corp"] {
            let folded = normalize(
                "url (https://user:hunter2@api.example.com:8443/v1/customers/acme-corp?key=sk_live_abc)",
            );
            assert!(!folded.contains(kept_out), "{kept_out} in {folded}");
        }
        // A numbered shard or tenant is one host, by the digit rule.
        assert_eq!(
            normalize("https://shop123.example.com/admin"),
            normalize("https://shop456.example.com/orders")
        );
        assert_eq!(normalize("https://api2.example.com/x"), "#.example.com/?");

        // No host, so nothing to keep: these fold whole, as before.
        for host_less in [
            // A module specifier in a stack frame: a scheme, but no authority.
            "file:///app/functions/upload-report/index.ts:12:34",
            "customer-app-storage/3fa85f64-5717-4562-b3fc-2c963f66afa6/uploads/invoice-acme.pdf",
            "/var/oxy/state/customer-app-storage/a/uploads/invoice-acme.pdf",
            "bob@example.com",
            // A URL in a query string is the caller's data, not where the
            // call went: there is no scheme in front of this run.
            "/redirect?to=https://evil.example/x",
        ] {
            assert_eq!(normalize(host_less), "?", "{host_less}");
        }
    }

    /// The mask one level further down. `url_host` keeps a host only where
    /// the message carries the URL bare, and reqwest 0.13 puts one on every
    /// send-phase error — but a body read that timed out or broke (`read
    /// failed: error decoding response body`, from `chunk()`, which carries
    /// no URL), a response over the cap, and the refusal that quotes its URL
    /// name no host, so a routine one on vendor A and a new one on vendor B
    /// were one fingerprint. The target the broker read off the call joins
    /// the fingerprint for exactly those. Nothing else moves: a message that
    /// names its host keeps the fingerprint `main` gave it (the literal,
    /// computed from the formula outside the crate), and a warehouse target
    /// never joins. Dropping the target from the fingerprint fails the first
    /// half; folding it in unconditionally fails the second.
    #[test]
    fn two_hosts_timeouts_in_one_function_are_two_fingerprints() {
        let body_timed_out = |host: &str| {
            HostCallFailure::noted(
                None,
                "fetch",
                "host_call_failed",
                "read failed: error decoding response body",
                Some(host),
            )
            .unwrap()
        };
        let vendor_a = body_timed_out("api.example.com");
        let vendor_b = body_timed_out("api.other.com");
        assert_eq!(
            vendor_a.message, vendor_b.message,
            "the message alone cannot tell the two apart"
        );
        assert_ne!(
            vendor_a.fingerprint(),
            vendor_b.fingerprint(),
            "a timeout on one vendor is new beside a routine one on another"
        );
        assert_eq!(
            vendor_a.fingerprint(),
            digest(
                "host_call fetch host_call_failed read failed: error decoding response body @ api.example.com"
            ),
            "the target joins the input after ` @ `, which no normalized message contains"
        );
        // The other two shapes the host writes with no host in them.
        for message in [
            "fetch to 'http://rates.example.com/v1/usd' blocked by SSRF allowlist",
            "response too large (exceeded the 10485760 byte cap)",
        ] {
            let a = HostCallFailure::noted(
                None,
                "fetch",
                "host_call_failed",
                message,
                Some("a.example.com"),
            );
            let b = HostCallFailure::noted(
                None,
                "fetch",
                "host_call_failed",
                message,
                Some("b.example.com"),
            );
            assert_ne!(
                a.unwrap().fingerprint(),
                b.unwrap().fingerprint(),
                "{message}"
            );
        }

        // "Names its host" means carries it as `url_host` writes one —
        // `<host>/?` — not that the target happens to be a substring: a
        // single-label host with a digit folds to `#`, which `# bytes`
        // contains, and the target still joins.
        let single_label = HostCallFailure::noted(
            None,
            "fetch",
            "host_call_failed",
            "response too large (10485761 bytes > 10485760 cap)",
            Some("svc2"),
        )
        .unwrap();
        assert_eq!(single_label.target.as_deref(), Some("#"));
        assert_eq!(
            single_label.fingerprint(),
            digest("host_call fetch host_call_failed response too large (# bytes > # cap) @ #")
        );

        // A message that names its host — every send-phase failure, a
        // timeout included — keeps the fingerprint `main` gave it: the
        // target is already in the input, so it is not added again.
        let named = HostCallFailure::noted(
            None,
            "fetch",
            "host_call_failed",
            "fetch failed: error sending request for url (https://api.example.com/v1/rates)",
            Some("api.example.com"),
        )
        .unwrap();
        assert_eq!(named.fingerprint(), "e019a17a15e9ebc7");
        assert_eq!(
            named.fingerprint(),
            HostCallFailure {
                target: None,
                ..named.clone()
            }
            .fingerprint()
        );
        // The target is compared normalized, as the message is, so a
        // numbered shard's host is found in the message that carries it.
        let shard = HostCallFailure::noted(
            None,
            "fetch",
            "host_call_failed",
            "fetch failed: error sending request for url (https://shop123.example.com/admin)",
            Some("shop123.example.com"),
        )
        .unwrap();
        assert_eq!(shard.target.as_deref(), Some("#.example.com"));
        assert_eq!(
            shard.fingerprint(),
            HostCallFailure {
                target: None,
                ..shard.clone()
            }
            .fingerprint()
        );

        // A warehouse target never joins: the caught Code 27 is one
        // fingerprint with a destination noted and without, so no stored
        // warehouse fingerprint moves.
        let with_target = HostCallFailure::noted(
            None,
            "warehouse.insert",
            "host_call_failed",
            CAUGHT_CODE_27,
            Some("dw ledger"),
        )
        .unwrap();
        let without = HostCallFailure::noted(
            None,
            "warehouse.insert",
            "host_call_failed",
            CAUGHT_CODE_27,
            None,
        )
        .unwrap();
        assert_eq!(with_target.fingerprint(), without.fingerprint());
        assert_eq!(
            with_target.fingerprint(),
            digest(&format!(
                "host_call warehouse.insert host_call_failed {}",
                normalized_prefix(CAUGHT_CODE_27)
            ))
        );
    }

    #[test]
    fn only_a_failed_outcome_is_a_failure() {
        assert_eq!(Failure::of("success", 200, None, None), None);
        assert_eq!(
            Failure::of("success", 404, None, None),
            None,
            "a 4xx is an answer"
        );
        assert_eq!(
            Failure::of("cancelled", 0, Some("function was cancelled"), None),
            None
        );

        let caught = Failure::of("success", 500, None, None).expect("a 5xx is a failure");
        assert_eq!(caught.kind, "http_5xx");
        assert_ne!(
            caught.fingerprint,
            Failure::of("success", 503, None, None).unwrap().fingerprint
        );

        assert_eq!(
            Failure::of("timeout", 0, Some("function execution timed out"), None)
                .unwrap()
                .kind,
            "timeout"
        );
        assert_eq!(
            Failure::of(
                "error",
                0,
                Some("internal runtime error: isolate died"),
                None
            )
            .unwrap()
            .kind,
            "internal"
        );
        let threw = Failure::of("error", 0, Some(CODE_27_A), None).unwrap();
        assert_eq!(threw.kind, "threw");
        assert_eq!(threw.fingerprint, fingerprint(CODE_27_B));
        // Never reached the app's code: the platform's failure, not the app's.
        assert_eq!(
            Failure::of(
                "error",
                0,
                Some("workspace lookup failed: connection refused"),
                None
            )
            .unwrap()
            .kind,
            "platform"
        );
        assert_eq!(
            Failure::of("timeout", 0, None, None).unwrap().fingerprint,
            Failure::timeout_fingerprint(),
            "the reaper's timeouts group with the runtime's"
        );
    }

    /// The Code 27 failure as `ctx.warehouse.insert` returned it to a handler
    /// that caught it — the same text minus the `function threw:` framing.
    const CAUGHT_CODE_27: &str = "warehouse insert failed: query failed: HTTP 400 \
        Bad Request: Code: 27. DB::Exception: Cannot parse input: expected '(' before: \
        '/*oxy.app=\\'bookkeeping\\',oxy.fn=\\'upload-report\\',oxy.invocation=\\'0af7651916cd\\'*/': \
        at row 63: While executing ValuesBlockInputFormat. (CANNOT_PARSE_INPUT_ASSERTION_FAILED) \
        (version 25.8.4.13 (official build))";

    #[test]
    fn a_caught_host_call_failure_on_a_2xx_is_a_failure() {
        let hc = HostCallFailure::noted(
            None,
            "warehouse.insert",
            "host_call_failed",
            CAUGHT_CODE_27,
            None,
        )
        .unwrap();
        let f = Failure::of("success", 200, None, Some(&hc)).expect("caught host failure pages");
        assert_eq!(f.kind, "host_call");
        assert_eq!(
            f.fingerprint,
            digest(&format!(
                "host_call warehouse.insert host_call_failed {}",
                normalized_prefix(CAUGHT_CODE_27)
            )),
            "the fingerprint is over the op, the kind and the normalized message, \
             bounded as the `threw` path's is"
        );
        assert_ne!(
            f.fingerprint,
            digest("host_call warehouse.insert host_call_failed"),
            "op and kind alone no longer name it"
        );
        assert_eq!(f.fingerprint.len(), 16);
        assert_eq!(f.host_call, Some(hc), "the page names the op and kind");
    }

    /// The mask this fixes. A function that already catches a routine failure
    /// on an op — `permission_denied` on a scheduled airhouse write, say — has
    /// that (op, kind) in the pager's lookback, so a real break of the same op
    /// and kind was not "new" and never paged. With the message in the
    /// fingerprint, the break is a fingerprint the function never had.
    #[test]
    fn a_new_failure_on_an_op_a_function_already_catches_is_a_new_fingerprint() {
        let routine = HostCallFailure::noted(
            None,
            "warehouse.insert",
            "host_call_failed",
            "warehouse insert failed: this is a read-only warehouse connection",
            None,
        )
        .unwrap();
        let broken = HostCallFailure::noted(
            None,
            "warehouse.insert",
            "host_call_failed",
            CAUGHT_CODE_27,
            None,
        )
        .unwrap();
        assert_eq!((routine.op, routine.kind), (broken.op, broken.kind));
        assert_ne!(
            Failure::of("success", 200, None, Some(&routine))
                .unwrap()
                .fingerprint,
            Failure::of("success", 200, None, Some(&broken))
                .unwrap()
                .fingerprint,
            "two failures of one op and kind are two fingerprints"
        );
        // And the same break across apps and rows is still one fingerprint —
        // the message is digested normalized, as the `threw` path's is.
        let other_app = HostCallFailure::noted(
            None,
            "warehouse.insert",
            "host_call_failed",
            &CAUGHT_CODE_27
                .replace("bookkeeping", "receiving")
                .replace("upload-report", "submit")
                .replace("0af7651916cd", "884e14953bc3")
                .replace("row 63", "row 2"),
            None,
        )
        .unwrap();
        assert_eq!(broken.fingerprint(), other_app.fingerprint());
    }

    /// The first failure stands over a later one, and a later success of the
    /// same op against the same target clears it: an app that retried and got
    /// its answer pages nobody. A success of another op clears nothing — a
    /// `storage.head` that worked says nothing about the `fetch` that did not.
    #[test]
    fn a_noted_host_call_failure_is_cleared_by_a_success_of_the_same_op() {
        let noted = HostCallFailure::noted(
            None,
            "fetch",
            "timeout",
            "fetch timed out after 30s",
            Some("api.example.com"),
        );
        assert_eq!(
            noted,
            Some(HostCallFailure {
                op: "fetch",
                kind: "timeout",
                message: "fetch timed out after #".into(),
                target: Some("api.example.com".into()),
            })
        );
        assert_eq!(
            HostCallFailure::noted(
                noted.clone(),
                "warehouse.insert",
                "host_call_failed",
                CAUGHT_CODE_27,
                None,
            ),
            noted,
            "the first failure stands"
        );
        assert_eq!(
            HostCallFailure::recovered(noted.clone(), "storage.head", None),
            noted
        );
        assert_eq!(
            HostCallFailure::recovered(noted.clone(), "fetch", Some("api.example.com")),
            None
        );
        // Recovered, then failed again: the second failure is the one the run
        // did not recover from.
        assert_eq!(
            HostCallFailure::noted(
                HostCallFailure::recovered(noted, "fetch", Some("api.example.com")),
                "warehouse.insert",
                "host_call_failed",
                CAUGHT_CODE_27,
                None,
            )
            .map(|hc| (hc.op, hc.kind)),
            Some(("warehouse.insert", "host_call_failed"))
        );
    }

    /// The mask the target closes. The fingerprint keeps a URL's host so two
    /// vendors under one `fetch` are two patterns, and a clear by op alone
    /// undid that: a caught failure on vendor A was dropped by any later
    /// `fetch` that worked — a Slack webhook, say — so A's break never paged.
    /// A success clears a failure only against the same target; an op with
    /// no target clears by op alone, as before.
    #[test]
    fn a_success_on_another_target_does_not_clear_a_noted_failure() {
        let vendor_a = HostCallFailure::noted(
            None,
            "fetch",
            "host_call_failed",
            "fetch failed: error sending request for url (https://api.vendor-a.com/v1/rates)",
            Some("api.vendor-a.com"),
        );
        assert_eq!(
            vendor_a.as_ref().and_then(|hc| hc.target.as_deref()),
            Some("api.vendor-a.com")
        );
        assert_eq!(
            HostCallFailure::recovered(vendor_a.clone(), "fetch", Some("hooks.slack.com")),
            vendor_a,
            "a fetch to another host says nothing about vendor A"
        );
        assert_eq!(
            HostCallFailure::recovered(vendor_a.clone(), "fetch", None),
            vendor_a,
            "a fetch with no host to compare is not the same target either"
        );
        assert_eq!(
            HostCallFailure::recovered(vendor_a, "fetch", Some("api.vendor-a.com")),
            None,
            "the same host, retried and answered, is recovered from"
        );

        // The target is compared as it was kept — normalized — so a numbered
        // shard is one host on both sides, as it is one input to the
        // fingerprint.
        let shard = HostCallFailure::noted(
            None,
            "fetch",
            "timeout",
            "fetch timed out after 30s",
            Some("shop123.example.com"),
        );
        assert_eq!(
            shard.as_ref().and_then(|hc| hc.target.as_deref()),
            Some("#.example.com")
        );
        assert_eq!(
            HostCallFailure::recovered(shard, "fetch", Some("shop456.example.com")),
            None
        );

        // A warehouse op keys on the destination and table it named: a
        // caught insert into `ledger` is not cleared by one into `audit`.
        let ledger = HostCallFailure::noted(
            None,
            "warehouse.insert",
            "host_call_failed",
            CAUGHT_CODE_27,
            Some("dw ledger"),
        );
        assert_eq!(
            HostCallFailure::recovered(ledger.clone(), "warehouse.insert", Some("dw audit")),
            ledger
        );
        assert_eq!(
            HostCallFailure::recovered(ledger, "warehouse.insert", Some("dw ledger")),
            None
        );
        // Numbered tables fold as numbered shards do: one target, by the
        // digit rule, so a success on `raw_v1` clears a failure on
        // `orders_2024` in the same database.
        let orders_2024 = HostCallFailure::noted(
            None,
            "warehouse.insert",
            "host_call_failed",
            CAUGHT_CODE_27,
            Some("dw orders_2024"),
        );
        assert_eq!(
            orders_2024.as_ref().and_then(|hc| hc.target.as_deref()),
            Some("dw #")
        );
        assert_eq!(
            HostCallFailure::recovered(orders_2024, "warehouse.insert", Some("dw raw_v1")),
            None
        );

        // An op with no target: op alone still clears.
        let query = HostCallFailure::noted(
            None,
            "query",
            "host_call_failed",
            "connection refused",
            None,
        );
        assert_eq!(HostCallFailure::recovered(query, "query", None), None);
    }

    /// What the host keeps across the run is the digest's input, never the
    /// message: normalized and bounded as it is noted.
    #[test]
    fn a_noted_message_is_normalized_before_it_is_kept() {
        let key =
            "customer-app-storage/3fa85f64-5717-4562-b3fc-2c963f66afa6/uploads/invoice-acme.pdf";
        let noted = HostCallFailure::noted(
            None,
            "storage.head",
            "host_call_failed",
            &format!("s3 error: head_object {key}: dispatch failure"),
            None,
        )
        .unwrap();
        assert_eq!(noted.message, "# error: head_object ?: dispatch failure");
        let long = format!(
            "warehouse insert failed: {}",
            "x ".repeat(FINGERPRINT_PREFIX)
        );
        let noted = HostCallFailure::noted(
            None,
            "warehouse.insert",
            "host_call_failed",
            &long,
            Some(&"y ".repeat(FINGERPRINT_PREFIX)),
        )
        .unwrap();
        assert_eq!(noted.message.chars().count(), FINGERPRINT_PREFIX);
        assert_eq!(
            noted.target.as_ref().map(|t| t.chars().count()),
            Some(FINGERPRINT_PREFIX),
            "the target is bounded as the message is"
        );
        assert_eq!(
            noted.fingerprint(),
            digest(&format!(
                "host_call warehouse.insert host_call_failed {}",
                noted.message
            ))
        );
    }

    /// "Nothing of the message reaches a log line" as a property of the type,
    /// not a habit of its callers: a `tracing::warn!(?failure)` added later
    /// prints the op, the kind and a digest.
    #[test]
    fn a_host_call_failures_debug_output_elides_its_message() {
        let hc = HostCallFailure::noted(
            None,
            "warehouse.insert",
            "permission_denied",
            "warehouse insert failed: permission denied for table ledger_entries",
            None,
        )
        .unwrap();
        let failure = Failure::of("success", 200, None, Some(&hc)).unwrap();
        for printed in [
            format!("{hc:?}"),
            format!("{hc:#?}"),
            format!("{failure:?}"),
        ] {
            assert!(printed.contains("warehouse.insert"), "{printed}");
            assert!(printed.contains("permission_denied"), "{printed}");
            for text in ["ledger_entries", "permission denied for", &hc.message] {
                assert!(!printed.contains(text), "{text} in {printed}");
            }
        }
        // The digest stands in for the text, so two notes still tell apart.
        let other =
            HostCallFailure::noted(None, "warehouse.insert", "permission_denied", "other", None)
                .unwrap();
        assert_ne!(format!("{hc:?}"), format!("{other:?}"));
    }

    #[test]
    fn a_host_call_failure_does_not_reclassify_a_real_failure() {
        let hc = HostCallFailure {
            op: "query",
            kind: "timeout",
            message: "query timed out after #".into(),
            target: None,
        };
        let real = Failure::of("success", 503, None, Some(&hc)).unwrap();
        assert_eq!(real.kind, "http_5xx");
        assert_eq!(real.host_call, None, "the page names the 5xx, not the call");
        assert_eq!(
            Failure::of("timeout", 0, None, Some(&hc)).unwrap().kind,
            "timeout"
        );
        assert_eq!(Failure::of("cancelled", 0, None, Some(&hc)), None);
    }

    /// Load shedding is the platform working as designed. Through the generic
    /// `error` path it raised a `Failure`, which marks the invocation span
    /// ERROR, writes a WARN on `oxy::app_function` and **pages** — reporting a
    /// capacity decision as the app failing, precisely when the fleet is
    /// busiest and a page storm is least welcome.
    #[test]
    fn a_shed_invocation_raises_no_failure_signal() {
        assert_eq!(
            Failure::of(
                "shed",
                0,
                Some(&super::super::runtime::RuntimeError::Overloaded.to_string()),
                None,
            ),
            None,
            "a shed must not page, mark the span ERROR, or dent the app's rate"
        );
    }

    /// The heap ceiling exists to make a tenant's runaway allocation
    /// *attributable*. Falling through to the `platform` default inverted that
    /// — on-call would see an Oxy fault for the one failure that is squarely
    /// the app's.
    ///
    /// Driven from the real `Display` string rather than a copy of it: the
    /// prefix match is coupled to that text, and a reworded error would
    /// otherwise silently reclassify to `platform` with every test still green.
    #[test]
    fn an_out_of_memory_error_is_attributed_to_the_app() {
        let message = super::super::runtime::RuntimeError::OutOfMemory.to_string();
        let failure = Failure::of("error", 0, Some(&message), None).expect("an OOM is a failure");
        assert_eq!(
            failure.kind, "exceeded_memory",
            "a heap breach must not read as a platform fault"
        );
    }

    /// The other two prefixes keep working — this is a closed list an alert
    /// facets on, so a new arm must not shadow an existing one.
    #[test]
    fn the_error_kinds_stay_distinct() {
        for (message, expected) in [
            ("function threw: TypeError", "threw"),
            ("internal runtime error: boom", "internal"),
            ("function exceeded its memory limit", "exceeded_memory"),
            ("workspace not found", "platform"),
        ] {
            assert_eq!(kind_of_error(message), expected, "for {message:?}");
        }
    }
}
