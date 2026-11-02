//! What the `ctx.*` ops do, with no JS engine in it.
//!
//! Every op is a function over [`OpContext`] (the state the isolate shares
//! with the broker) that takes and returns strings, booleans or JSON text. An
//! engine's binding layer — today `engine::deno::ops`, the `#[op2]` glue —
//! only unpacks the engine's arguments, calls in here, and converts an
//! [`OpError`] into the engine's own JS error. Keeping the logic on this side
//! is what lets a second engine bind the same ops without re-deriving a
//! single rule (the log cap, the cancellation check, the error envelope, the
//! HMAC refusals).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{mpsc, oneshot};

use super::super::LogLine;
use super::super::host_error::HostError;
use super::host_call::HostCall;

/// An op that failed. Surfaces in JS as a plain `Error` whose message is this
/// text — the engine binding must not add to it (`JsErrorBox::generic` today).
#[derive(Debug)]
pub(super) struct OpError(pub String);

impl OpError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The state every op shares for one invocation: the channel to the broker,
/// the cancellation flag, and the log buffer. An engine parks one of these in
/// its per-isolate op state when it creates the isolate.
pub(super) struct OpContext {
    pub call_tx: mpsc::UnboundedSender<HostCall>,
    pub cancelled: Arc<AtomicBool>,
    pub logs: FunctionLogs,
}

/// What `op_ctx_log` should do with a line, given how many are already buffered.
///
/// Pure so the cap can be tested without a V8 isolate — and it needed testing:
/// the first version returned at the marker without pushing, so the buffer
/// parked at exactly `MAX_CAPTURED_LOGS`, `Silent` was unreachable, and the
/// marker re-fired once per suppressed line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LogAction {
    /// Under the cap — record it.
    Emit,
    /// Exactly at the cap — emit one truncation marker and record THAT, so the
    /// next call is over the cap rather than at it.
    Marker,
    /// Over the cap — drop silently.
    Silent,
}

pub(super) fn log_action(buffered: usize) -> LogAction {
    match buffered.cmp(&MAX_CAPTURED_LOGS) {
        std::cmp::Ordering::Less => LogAction::Emit,
        std::cmp::Ordering::Equal => LogAction::Marker,
        std::cmp::Ordering::Greater => LogAction::Silent,
    }
}

/// Per-invocation log buffer, shared between the isolate thread (appends via
/// `op_ctx_log`) and the caller (drains it after the run). Capped so a runaway
/// loop can't OOM.
pub(super) struct FunctionLogs(pub Arc<std::sync::Mutex<Vec<LogLine>>>);
pub(super) const MAX_CAPTURED_LOGS: usize = 500;

impl OpContext {
    /// `ctx.log` / `console.*` — `op_ctx_log`.
    pub fn log(&self, level: &str, message: &str) {
        // `name` is the field the observability layer's `EventVisitor` promotes to
        // the event name, so these land in ClickHouse as an identifiable
        // `function_log` rather than under the message's own text. `log_level`
        // survives the trip too — the stored event record keeps fields and an
        // error flag, not the tracing `Level`, so warn and info would otherwise be
        // indistinguishable once written.
        //
        // Nothing here names the app, function or invocation: that context comes
        // from the enclosing span opened by `run_with_runtime`, which `run` carries
        // onto the isolate thread. Without that span these events are dropped
        // entirely — `SpanCollectorLayer::on_event` returns early when there is no
        // current span.
        // The emit is capped by the SAME budget as the returned buffer, and that
        // ordering matters. With a span now enclosing these, every event is
        // retained in span extensions and serialised into one `event_data` blob at
        // close — so an uncapped emit turns `for (i=0;i<100000;i++) console.log()`
        // into unbounded per-invocation memory and a single enormous row.
        // Observability needs hard size caps (product-context is explicit), and
        // reusing the existing limit means the buffer and the span agree about
        // what was kept.
        let FunctionLogs(buf) = &self.logs;
        // Poisoned mutex: drop the line rather than emit it uncapped. A
        // poisoned lock means another thread panicked mid-push — at which
        // point an uncapped emit is the wrong direction to fail in.
        let Some(count) = buf.lock().ok().map(|v| v.len()) else {
            return;
        };
        let action = log_action(count);
        if action == LogAction::Silent {
            return;
        }
        if action == LogAction::Marker {
            // One marker, not silence: a truncated log that says so is debuggable,
            // one that just stops is misleading.
            //
            // The marker is PUSHED as well as emitted, and that is what makes the
            // cap a cap. Returning without pushing left the buffer parked at
            // exactly MAX forever, so `count == MAX` on every subsequent call and
            // the marker re-fired per suppressed line — 99,500 identical warn
            // events for a 100k-line loop, each retained in the span until close.
            // Pushing takes the length to MAX + 1, so every later call takes the
            // silent `>` branch above, and the buffer the app receives ends by
            // saying it was truncated instead of just stopping.
            let marker = format!("… further output suppressed after {MAX_CAPTURED_LOGS} lines");
            tracing::warn!(
                target: "custom_app_function",
                name = "function_log",
                log_level = "warn",
                "{marker}"
            );
            if let Ok(mut v) = buf.lock() {
                v.push(LogLine {
                    level: "warn".to_string(),
                    message: marker,
                });
            }
            return;
        }

        match level {
            "warn" => {
                tracing::warn!(target: "custom_app_function", name = "function_log", log_level = "warn", "{message}")
            }
            "error" => {
                tracing::error!(target: "custom_app_function", name = "function_log", log_level = "error", "{message}")
            }
            _ => {
                tracing::info!(target: "custom_app_function", name = "function_log", log_level = "info", "{message}")
            }
        }
        if let Ok(mut v) = buf.lock() {
            v.push(LogLine {
                level: level.to_string(),
                message: message.to_string(),
            });
        }
    }

    /// Second cancellation layer (design doc §11.4): checked at the entry of
    /// every `ctx.*` op so an in-flight or about-to-start host call fails fast
    /// once cancellation has been observed, rather than only relying on
    /// terminating the isolate (which interrupts JS execution but not an
    /// already-dispatched host call).
    pub fn check_cancelled(&self) -> Result<(), OpError> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(OpError::new("function was cancelled"));
        }
        Ok(())
    }

    /// The first half of every async `ctx.*` op: the cancellation check, then a
    /// handle on the broker channel. Split from [`HostSender::call`] so an
    /// engine can release its borrow of the op state before awaiting.
    pub fn host_sender(&self) -> Result<HostSender, OpError> {
        self.check_cancelled()?;
        Ok(HostSender(self.call_tx.clone()))
    }
}

/// [`OpContext::host_sender`] for an op that may run before any invocation is
/// bound (`IsolateEngine::bind_invocation`): the refusal is the one a dropped
/// broker gives, so an op reaching an isolate with nothing bound fails that
/// call instead of panicking the isolate thread.
pub(super) fn host_sender_of(ctx: Option<&OpContext>) -> Result<HostSender, OpError> {
    ctx.ok_or_else(|| OpError::new("function host unavailable"))?
        .host_sender()
}

/// A clone of the broker channel, taken by [`OpContext::host_sender`].
pub(super) struct HostSender(mpsc::UnboundedSender<HostCall>);

/// The reply end every [`HostCall`] carries.
pub(super) type HostReply = oneshot::Sender<Result<serde_json::Value, HostError>>;

impl HostSender {
    /// Send one host call to the broker and wait for its answer, encoded as
    /// the JSON envelope the bootstrap `__wrapOp` reads (see [`reply_json`]).
    /// `what` is the surface name (`ctx.query`, …).
    pub async fn call(
        self,
        what: &str,
        make: impl FnOnce(HostReply) -> HostCall,
    ) -> Result<String, OpError> {
        let (reply, rx) = oneshot::channel();
        self.0
            .send(make(reply))
            .map_err(|_| OpError::new("function host unavailable"))?;
        let result = rx
            .await
            .map_err(|_| OpError::new("function host dropped the request"))?;
        Ok(reply_json(what, result))
    }
}

/// A JSON-stringified op argument. Anything that does not parse is `null`, as
/// every op but `ctx.semantic.query` has always treated it.
pub(super) fn parse_payload(json: &str) -> serde_json::Value {
    serde_json::from_str(json).unwrap_or(serde_json::Value::Null)
}

/// `ctx.semantic.query`'s spec, which — unlike every other payload — must parse.
pub(super) fn parse_semantic_spec(json: &str) -> Result<serde_json::Value, OpError> {
    serde_json::from_str(json)
        .map_err(|e| OpError::new(format!("invalid semantic query spec: {e}")))
}

/// Encode a host reply as the JSON envelope the bootstrap `__wrapOp` reads:
/// either the value itself, or `{ __oxyError, message }` on failure.
///
/// `what` is the surface name (`ctx.query`, `ctx.oltp`, …) and this is its ONE
/// owner: host methods, the transaction registry and the Postgres connector all
/// return BARE messages, and the prefix is added here. They used to spell it
/// themselves as well, which rendered `ctx.oltp: ctx.oltp query: query failed:
/// ctx.tx: db error` — the surface named three times, the cause not once. The
/// connector is the reason this has to be a rule rather than a habit: it backs
/// both `ctx.tx` and `ctx.oltp` and cannot know which one called it.
pub(super) fn reply_json(what: &str, result: Result<serde_json::Value, HostError>) -> String {
    match result {
        Ok(value) => value.to_string(),
        // The one place the typed error becomes the string the isolate throws:
        // `Display` is the text, byte for byte, so the JS-visible message and
        // the audit rows are what they were when this was a `String`.
        Err(error) => serde_json::json!({
            "__oxyError": super::super::host_error::HOST_ERROR_NAME,
            "message": format!("{what}: {error}"),
        })
        .to_string(),
    }
}

/// HMAC over `data` with `key`, both read as UTF-8.
///
/// UTF-8 only, deliberately: every webhook scheme in the wild signs a UTF-8
/// base string with a UTF-8 secret (GitHub signs the body, Slack `v0:ts:body`,
/// Stripe `ts.body`). A binary key or payload would need an encoding knob per
/// argument, which is surface nobody has asked for.
pub(super) fn hmac_digest(algorithm: &str, key: &str, data: &str) -> Result<Vec<u8>, OpError> {
    use hmac::{Hmac, KeyInit, Mac};
    // An unset or empty secret must not silently become a usable key: an
    // attacker can sign with "" as easily as we can. This is enforced here, not
    // only in the JS wrapper, because the artifact shares a global with the
    // bootstrap and can reach `Deno.core.ops` directly.
    if key.is_empty() {
        return Err(OpError::new(
            "ctx.crypto: `key` must not be empty — check the secret is set",
        ));
    }
    let bad_key = || OpError::new("ctx.crypto: key is not valid for this algorithm");
    match algorithm {
        "sha256" => {
            let mut mac =
                Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes()).map_err(|_| bad_key())?;
            mac.update(data.as_bytes());
            Ok(mac.finalize().into_bytes().to_vec())
        }
        "sha512" => {
            let mut mac =
                Hmac::<sha2::Sha512>::new_from_slice(key.as_bytes()).map_err(|_| bad_key())?;
            mac.update(data.as_bytes());
            Ok(mac.finalize().into_bytes().to_vec())
        }
        other => Err(OpError::new(format!(
            "ctx.crypto: unknown algorithm '{other}' (expected 'sha256' or 'sha512')"
        ))),
    }
}

fn encode_digest(bytes: &[u8], encoding: &str) -> Result<String, OpError> {
    use base64::Engine as _;
    match encoding {
        "hex" => Ok(hex::encode(bytes)),
        "base64" => Ok(base64::engine::general_purpose::STANDARD.encode(bytes)),
        other => Err(OpError::new(format!(
            "ctx.crypto: unknown encoding '{other}' (expected 'hex' or 'base64')"
        ))),
    }
}

/// `ctx.crypto.hmac` — sign. For talking TO an API that requires a signed
/// request; the inverse direction from `verifyHmac`.
pub(super) fn hmac(
    algorithm: &str,
    key: &str,
    data: &str,
    encoding: &str,
) -> Result<String, OpError> {
    encode_digest(&hmac_digest(algorithm, key, data)?, encoding)
}

/// `ctx.crypto.verifyHmac` — the reason the crypto ops exist. The comparison
/// is constant-time via [`constant_time_eq`] below; it does NOT use
/// `Mac::verify_slice`, because the provided signature has to be decoded from
/// hex/base64 first and a decode failure must reject rather than throw.
///
/// **A signature that will not decode returns `false`, it does not throw.**
/// That string is attacker-controlled: throwing would turn a forged request
/// into a 500 and an alert, instead of a clean rejection. An unknown
/// `algorithm` or `encoding` DOES throw, because those come from the app
/// author, not the caller.
///
/// The app strips any provider prefix first (`sha256=`, `v0=`) and passes the
/// bare digest — prefix formats are per-provider and do not belong in the
/// platform.
pub(super) fn verify_hmac(
    algorithm: &str,
    key: &str,
    data: &str,
    signature: &str,
    encoding: &str,
) -> Result<bool, OpError> {
    use base64::Engine as _;
    // Validate author-supplied inputs first so a bad algorithm throws even when
    // the signature is also junk.
    let expected = hmac_digest(algorithm, key, data)?;
    let provided = match encoding {
        "hex" => hex::decode(signature).ok(),
        "base64" => base64::engine::general_purpose::STANDARD
            .decode(signature)
            .ok(),
        other => {
            return Err(OpError::new(format!(
                "ctx.crypto: unknown encoding '{other}' (expected 'hex' or 'base64')"
            )));
        }
    };
    let Some(provided) = provided else {
        return Ok(false);
    };
    Ok(constant_time_eq(&expected, &provided))
}

/// `ctx.crypto.timingSafeEqual` — for a plain shared secret carried in a
/// header, where there is no HMAC to verify. Without it an author writes
/// `a === b`, which leaks the secret one byte at a time.
pub(super) fn timing_safe_equal(a: &str, b: &str) -> bool {
    // An empty side means "absent" — an unset secret, or a header the caller
    // omitted. `constant_time_eq(b"", b"")` is true, so without this the
    // documented pattern
    // `timingSafeEqual(req.headers[...], ctx.env.SECRET)` AUTHORIZES when the
    // secret was never configured and the attacker sends nothing. Fail closed.
    if a.is_empty() || b.is_empty() {
        return false;
    }
    constant_time_eq(a.as_bytes(), b.as_bytes())
}

/// Length is not secret here (a digest length is fixed by the algorithm, and a
/// shared secret's length is not the part worth protecting), but the byte
/// comparison must not short-circuit.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

/// The request body, parked in the op state for the invoke script to collect.
///
/// It is handed over by `op_req_take_body` rather than written into the
/// invoke script as a literal. V8 keeps a script's source for as long as the
/// script runs, so a body spliced into it sat on the isolate's heap twice
/// before the handler's first line — once as source, once as the literal's
/// value — and both counted against `heap_limit_bytes`. For the largest body
/// the route accepts that was half the default ceiling.
///
/// `op_req_take_body` hands it over once; any later call gets an empty string.
/// The artifact shares a global with the bootstrap, so app code can reach that
/// op too. The invoke script calls it before anything else, and an app whose
/// top-level code has replaced `Deno.core.ops` can lose only its own request's
/// body by doing so.
pub(super) struct RequestBody(pub String);
