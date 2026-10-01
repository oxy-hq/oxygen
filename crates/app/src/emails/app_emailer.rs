//! App-facing email sending for Oxy Functions (`ctx.email.send`).
//!
//! Sits on top of the SES transport in this module. The **platform** controls
//! the `from` mailbox — a function author may set `replyTo` only, never `from`.
//! Recipients may be arbitrary external addresses, bounded by a per-send
//! recipient cap. In local/dev the email is logged instead of sent.
//!
//! Design: `internal-docs/customer-apps-functions.md`.

use aws_sdk_sesv2::types::{Destination, EmailContent, RawMessage};
use serde::Deserialize;

mod redirect;
pub use redirect::IgnoredRecipients;

/// Max combined `to` + `cc` + `bcc` recipients per `ctx.email.send` call
/// (Cloudflare's number). Bounds fan-out from a single send.
pub const MAX_RECIPIENTS_PER_SEND: usize = 50;

/// Max combined **decoded** attachment bytes per send. Sits comfortably under
/// SES's ~40 MB total message ceiling and under the 32 MiB custom-app request
/// body limit (a base64 payload inflates ~33% in transit, so ~10 MiB decoded is
/// ~13.4 MiB on the wire). Anything larger should be stored via `ctx.storage`
/// and emailed as a presigned link instead of inlined.
pub const MAX_ATTACHMENT_TOTAL_BYTES: usize = 10 * 1024 * 1024;

/// Max attachment count per send — bounds work per message independently of the
/// byte cap (a thousand 1-byte parts is also a bad message).
pub const MAX_ATTACHMENTS_PER_SEND: usize = 20;

/// Default platform sender mailbox when `OXY_APP_EMAIL_FROM` is unset. Must be a
/// verified SES identity in the target account.
const DEFAULT_FROM_MAILBOX: &str = "noreply@oxygen-hq.com";

/// `string | string[]` as it arrives from the JS payload.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    fn into_vec(self) -> Vec<String> {
        match self {
            OneOrMany::One(s) => vec![s],
            OneOrMany::Many(v) => v,
        }
    }
}

/// Author-supplied payload for `ctx.email.send`, deserialized from the JS object
/// the isolate passes through the host op. `from` is captured only so it can be
/// **rejected** — the platform owns the sender address.
#[derive(Debug, Deserialize)]
pub struct EmailSendInput {
    #[serde(default)]
    to: Option<OneOrMany>,
    #[serde(default)]
    cc: Option<OneOrMany>,
    #[serde(default)]
    bcc: Option<OneOrMany>,
    #[serde(rename = "replyTo", default)]
    reply_to: Option<String>,
    #[serde(default)]
    subject: Option<String>,
    #[serde(default)]
    html: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(rename = "idempotencyKey", default)]
    idempotency_key: Option<String>,
    /// Files to attach, base64-encoded by the author. Bounded by
    /// [`MAX_ATTACHMENTS_PER_SEND`] and [`MAX_ATTACHMENT_TOTAL_BYTES`].
    #[serde(default)]
    attachments: Option<Vec<EmailAttachmentInput>>,
    /// Not an accepted field — captured only to reject it explicitly.
    #[serde(default)]
    from: Option<String>,
}

/// One author-supplied attachment. `content` defaults to base64 (the only way
/// binary can cross the isolate's JSON op boundary); it is decoded and
/// size-checked here.
#[derive(Debug, Deserialize)]
pub struct EmailAttachmentInput {
    #[serde(default)]
    filename: Option<String>,
    #[serde(default)]
    content: Option<String>,
    /// How `content` is encoded: `"base64"` (default) or `"utf8"`. Mirrors
    /// `ctx.storage.put`. `"utf8"` exists because the overwhelmingly common
    /// attachment is text a function just generated (CSV/JSON/HTML) — and
    /// `btoa` cannot encode a non-ASCII string, so forcing base64 either
    /// corrupts it or makes the author hand-roll UTF-8 encoding.
    #[serde(default)]
    encoding: Option<String>,
    #[serde(rename = "contentType", default)]
    content_type: Option<String>,
    /// Render inline (e.g. an image referenced by `cid:`) rather than as a
    /// downloadable attachment. Defaults to false.
    #[serde(default)]
    inline: Option<bool>,
    /// Content-ID for an inline part, referenced from the HTML as `cid:<id>`.
    #[serde(rename = "contentId", default)]
    content_id: Option<String>,
}

/// Sends email on behalf of a custom app's function. Platform-controlled
/// sender; SES transport in cloud, `tracing` log in local/dev. Cheap to
/// construct (reads env only); the SES client is built lazily per send.
pub struct AppEmailer {
    /// Friendly display name for the `From` header (the app's name).
    app_name: String,
    /// The verified platform mailbox all app email is sent from.
    from_mailbox: String,
    /// Region for the SES client (`None` → resolved from the AWS env chain).
    aws_region: Option<String>,
    /// When true, log the composed email instead of calling SES (local/dev).
    local_test: bool,
}

impl AppEmailer {
    /// Build from process env + the app's display name.
    ///
    /// - `OXY_APP_EMAIL_FROM` — sender mailbox (default `noreply@oxygen-hq.com`).
    /// - `OXY_APP_EMAIL_REGION` — SES region (else the default AWS env chain).
    ///
    /// Local mode (see `serve_mode`) defaults to the browser live-preview —
    /// never SES on a laptop; cloud sends via SES. `OXY_APP_EMAIL_LOCAL_TEST`
    /// optionally overrides (`0`/`false` → force SES; any other value → force
    /// preview).
    pub fn from_env(app_name: impl Into<String>) -> Self {
        let from_mailbox = std::env::var("OXY_APP_EMAIL_FROM")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_FROM_MAILBOX.to_string());
        let aws_region = std::env::var("OXY_APP_EMAIL_REGION")
            .ok()
            .filter(|s| !s.trim().is_empty());
        // Default to the browser live-preview in local mode — never call SES on
        // a dev laptop; cloud sends for real. The env var is an optional
        // override (`0`/`false` → SES, any other value → preview).
        let local_test = match std::env::var("OXY_APP_EMAIL_LOCAL_TEST") {
            Ok(v) if !v.trim().is_empty() => !matches!(v.trim(), "0" | "false" | "no"),
            _ => oxy_app_core::serve_mode::process_is_local().unwrap_or(false),
        };
        Self {
            app_name: app_name.into(),
            from_mailbox,
            aws_region,
            local_test,
        }
    }

    /// Validate + send. Returns the value `ctx.email.send` resolves to
    /// (`{ "messageId": ... }`), or `Err(message)` surfaced to JS via the
    /// runtime's `__oxyError` envelope. Error messages are prefixed with a
    /// typed label (`SenderRejected`, `TooManyRecipients`, …).
    pub async fn send(&self, input: EmailSendInput) -> Result<serde_json::Value, String> {
        let msg = self.validate(input)?;
        // Composed BEFORE the local/cloud fork, and propagated either way. A
        // payload that cannot produce a valid message (an address lettre
        // rejects, a misconfigured platform sender) must fail identically on a
        // laptop and in cloud — a preview that logged the error and returned a
        // `messageId` anyway would reopen, one layer up, exactly the
        // dev-vs-prod blind spot that let the 7bit bug ship.
        let mime = self.compose(&msg)?;
        // Local mode (or the `OXY_APP_EMAIL_LOCAL_TEST` override) previews in the
        // browser and never touches SES. In cloud, SES errors PROPAGATE: a
        // transient failure (DispatchFailure, credential rotation) must re-run via
        // the durable queue — never get swallowed behind a fake `messageId`. A
        // cloud dev box that wants the preview sets `OXY_APP_EMAIL_LOCAL_TEST=1`
        // (the SES-config error message says so).
        if self.local_test {
            return Ok(self.preview_local(&msg, &mime));
        }
        self.send_ses(&msg, mime).await
    }

    /// Assemble the RFC-5322 bytes for `msg`, sender included.
    ///
    /// The display name is passed unencoded alongside the bare mailbox rather
    /// than as a preformatted `Name <addr>` string — see `mime::build_mime`.
    fn compose(&self, msg: &ValidatedEmail) -> Result<Vec<u8>, String> {
        super::mime::build_mime(
            sanitized_display_name(&self.app_name).as_deref(),
            &self.from_mailbox,
            msg,
        )
    }

    /// Validate the payload into a normalized, ready-to-send message.
    fn validate(&self, input: EmailSendInput) -> Result<ValidatedEmail, String> {
        if input.from.is_some() {
            return Err(
                "InvalidEmailPayload: `from` is not settable — the platform \
                        controls the sender address; use `replyTo` instead"
                    .to_string(),
            );
        }
        let subject = input
            .subject
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or("InvalidEmailPayload: `subject` is required")?
            .to_string();
        // Trimmed and de-blanked here so both consumers see the same list:
        // lettre's parser rejects a leading space outright, and SES would take
        // it into the envelope. `" a@b.com "` out of a form field is ordinary
        // input, not a payload bug worth failing a send over.
        let to = addresses(input.to);
        let cc = addresses(input.cc);
        let bcc = addresses(input.bcc);
        check_recipients(&to, &cc, &bcc)?;
        if input.html.is_none() && input.text.is_none() {
            return Err("InvalidEmailPayload: provide `html` or `text`".to_string());
        }
        if input
            .idempotency_key
            .as_deref()
            .is_some_and(|k| k.len() > 256)
        {
            return Err("InvalidEmailPayload: `idempotencyKey` exceeds 256 characters".to_string());
        }
        let attachments = validate_attachments(input.attachments.unwrap_or_default())?;
        Ok(ValidatedEmail {
            subject,
            to,
            cc,
            bcc,
            reply_to: input
                .reply_to
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
            html: input.html,
            text: input.text,
            attachments,
        })
    }

    /// Dev path: instead of sending, write the composed email into a dedicated
    /// `oxy-email-previews/` subdir of the temp dir (so it doesn't litter temp;
    /// auto-pruned) and open it in the browser so a developer sees the rendered
    /// template with its real data. Returns a synthetic message id. Prefers HTML.
    fn preview_local(&self, msg: &ValidatedEmail, mime: &[u8]) -> serde_json::Value {
        let message_id = format!("local-test-{}", uuid::Uuid::new_v4());
        // Prefer the HTML body for a faithful preview; fall back to text.
        let (mut contents, ext) = match (&msg.html, &msg.text) {
            (Some(html), _) => (html.clone(), "html"),
            (None, Some(text)) => (text.clone(), "txt"),
            (None, None) => (String::new(), "txt"),
        };
        // The preview doesn't send, so attachments would otherwise be invisible —
        // list them so a dev can confirm what WOULD have been attached.
        if !msg.attachments.is_empty() {
            contents.push_str(&attachment_manifest(&msg.attachments, ext));
        }
        let dir = preview_dir();
        prune_old_previews(&dir);
        let path = dir.join(format!("email-{message_id}.{ext}"));
        tracing::info!(
            from = %self.from_mailbox,
            to = ?msg.to,
            cc = ?msg.cc,
            bcc = ?msg.bcc,
            reply_to = ?msg.reply_to,
            subject = %msg.subject,
            %message_id,
            path = %path.display(),
            "email preview (local mode / SES not configured): not sending; opening rendered email"
        );
        match std::fs::write(&path, contents) {
            Ok(()) => super::local_test::open_in_browser(&path),
            Err(e) => tracing::warn!("failed to write email preview to {}: {e}", path.display()),
        }
        self.write_eml_preview(msg, mime, &message_id, &dir);
        serde_json::json!({ "messageId": message_id })
    }

    /// Write the byte-exact message SES would receive next to the rendered
    /// preview, as `.eml`.
    ///
    /// The HTML preview above shows the *template*; it says nothing about
    /// transfer encodings or part structure, which is why a release that
    /// mangled every binary attachment looked perfect on a laptop. This file
    /// opens in any mail client, so "is the attachment actually intact?" is
    /// answerable before merge instead of after a customer says so.
    ///
    /// The bytes are composed by the caller, so a composition failure has
    /// already aborted the send; only the filesystem write is best-effort here.
    fn write_eml_preview(
        &self,
        msg: &ValidatedEmail,
        mime: &[u8],
        message_id: &str,
        dir: &std::path::Path,
    ) {
        let path = dir.join(format!("email-{message_id}.eml"));
        match std::fs::write(&path, mime) {
            Ok(()) => tracing::info!(
                path = %path.display(),
                attachments = msg.attachments.len(),
                "wrote the exact MIME SES would receive; open it in a mail client to \
                 verify attachments end-to-end"
            ),
            Err(e) => tracing::warn!("failed to write .eml preview to {}: {e}", path.display()),
        }
    }

    /// Cloud path: send via SES v2, returning the SES message id.
    async fn send_ses(
        &self,
        msg: &ValidatedEmail,
        mime: Vec<u8>,
    ) -> Result<serde_json::Value, String> {
        let client = ses_client(self.aws_region.as_deref()).await;
        let out = self
            .build_send_email(client, msg, mime)?
            .send()
            .await
            .map_err(|e| classify_ses_error(error_detail(&e)))?;
        let message_id = out.message_id().unwrap_or_default().to_string();
        Ok(serde_json::json!({ "messageId": message_id }))
    }

    /// Compose the SES `SendEmail` request around MIME **we** built.
    ///
    /// The message goes out as `Content.Raw`, not `Content.Simple`. Letting SES
    /// assemble the MIME put the encoding decision on a server this process
    /// cannot observe, and its undocumented default (`7bit`) destroyed every
    /// binary attachment. See `emails/mime.rs` for the full account.
    ///
    /// Recipients are still passed as a `Destination`: those are the envelope
    /// addresses SES actually delivers to, and it is how a Bcc reaches its
    /// recipient without appearing in the headers.
    fn build_send_email(
        &self,
        client: &aws_sdk_sesv2::Client,
        msg: &ValidatedEmail,
        mime: Vec<u8>,
    ) -> Result<aws_sdk_sesv2::operation::send_email::builders::SendEmailFluentBuilder, String>
    {
        let mut dest = Destination::builder().set_to_addresses(Some(msg.to.clone()));
        if !msg.cc.is_empty() {
            dest = dest.set_cc_addresses(Some(msg.cc.clone()));
        }
        if !msg.bcc.is_empty() {
            dest = dest.set_bcc_addresses(Some(msg.bcc.clone()));
        }

        let raw = RawMessage::builder()
            .data(aws_sdk_sesv2::primitives::Blob::new(mime))
            .build()
            .map_err(|e| format!("InvalidEmailPayload: could not build the raw message: {e}"))?;

        // `Reply-To` is a header inside the MIME now, so it is NOT also set on
        // the request — SES would add a second one and the recipient's client
        // would have to guess.
        Ok(client
            .send_email()
            // The BARE mailbox, not `Name <mailbox>`. SES uses this as the
            // source identity (and the bounce path); the display name belongs
            // to the MIME `From` header, which lettre owns. Rendering the name
            // here too would mean two escapers for one value — the SES field
            // carrying raw UTF-8 while the header carries RFC-2047 — and only
            // one of them is covered by the MIME tests.
            .from_email_address(&self.from_mailbox)
            .destination(dest.build())
            .content(EmailContent::builder().raw(raw).build()))
    }
}

/// A validated, normalized email ready to hand to SES or the local logger.
#[derive(Debug)]
pub(super) struct ValidatedEmail {
    pub(super) subject: String,
    pub(super) to: Vec<String>,
    pub(super) cc: Vec<String>,
    pub(super) bcc: Vec<String>,
    pub(super) reply_to: Option<String>,
    pub(super) html: Option<String>,
    pub(super) text: Option<String>,
    pub(super) attachments: Vec<ValidatedAttachment>,
}

/// An attachment whose base64 has been decoded and size-checked.
#[derive(Debug)]
pub(super) struct ValidatedAttachment {
    pub(super) filename: String,
    pub(super) bytes: Vec<u8>,
    pub(super) content_type: Option<String>,
    pub(super) inline: bool,
    pub(super) content_id: Option<String>,
}

/// Decode + bound the author's attachments. Enforces the count cap, the
/// **decoded** total-byte cap, a non-empty header-safe filename, and rejects
/// malformed base64 loudly rather than silently sending an empty part.
fn validate_attachments(
    inputs: Vec<EmailAttachmentInput>,
) -> Result<Vec<ValidatedAttachment>, String> {
    if inputs.len() > MAX_ATTACHMENTS_PER_SEND {
        return Err(format!(
            "TooManyAttachments: {} attachments exceeds the per-send limit of \
             {MAX_ATTACHMENTS_PER_SEND}",
            inputs.len()
        ));
    }
    let mut out = Vec::with_capacity(inputs.len());
    let mut total = 0usize;
    for (idx, a) in inputs.into_iter().enumerate() {
        let filename = a
            .filename
            .as_deref()
            .map(attachment_filename)
            .filter(|f| !f.is_empty())
            .ok_or_else(|| {
                format!("InvalidEmailPayload: attachment[{idx}] requires a `filename`")
            })?;
        let content = a.content.ok_or_else(|| {
            // Deliberately does NOT say "base64": with `encoding: "utf8"` the
            // author needs no encoder, and naming base64 here would send them
            // hunting for one.
            format!("InvalidEmailPayload: attachment[{idx}] ('{filename}') requires `content`")
        })?;
        // An absent, empty, or whitespace-only `encoding` means "unspecified" →
        // the default. `?? ""` in JS is an easy way to produce the empty case,
        // and it should not be a distinct error from omitting the field.
        let encoding = a
            .encoding
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("base64");
        let bytes = match encoding {
            "base64" => {
                // Ignore whitespace/newlines a caller may have wrapped the base64 in.
                let compact: String = content.chars().filter(|c| !c.is_whitespace()).collect();
                base64_decode(&compact).map_err(|e| {
                    format!("InvalidEmailPayload: attachment[{idx}] ('{filename}') `content` is not valid base64: {e}")
                })?
            }
            // Text the function generated — attach the bytes verbatim.
            "utf8" => content.into_bytes(),
            other => {
                return Err(format!(
                    "InvalidEmailPayload: attachment[{idx}] ('{filename}') unknown `encoding` \
                     '{other}' (expected 'base64' or 'utf8')"
                ));
            }
        };
        // An empty `content` is *valid* input to both decoders — base64 "" and
        // utf8 "" each yield zero bytes — so neither guard above fires and SES
        // is handed a well-formed part with the right filename, type and
        // disposition wrapped around nothing. That is delivered as a 0-byte
        // file the recipient's viewer calls corrupt, with no error anywhere on
        // our side. Whatever produced the empty string (`?? ""`, a read that
        // returned nothing, a mis-keyed field) is a bug in the caller, so say
        // so at the boundary instead of mailing the evidence to a customer.
        if bytes.is_empty() {
            return Err(format!(
                "InvalidEmailPayload: attachment[{idx}] ('{filename}') is empty — `content` \
                 decoded to zero bytes. An empty part is delivered as a corrupt file; check \
                 that the value passed to `content` is non-empty (a `?? \"\"` fallback, or a \
                 ctx.storage/ctx.fetch read that returned nothing, both land here)"
            ));
        }
        total = total.saturating_add(bytes.len());
        if total > MAX_ATTACHMENT_TOTAL_BYTES {
            return Err(format!(
                "AttachmentTooLarge: attachments total {total} bytes, over the \
                 {MAX_ATTACHMENT_TOTAL_BYTES}-byte per-send limit; store the file with \
                 ctx.storage and email a presigned link instead"
            ));
        }
        out.push(ValidatedAttachment {
            filename,
            bytes,
            content_type: a.content_type.filter(|s| !s.trim().is_empty()),
            inline: a.inline.unwrap_or(false),
            content_id: a
                .content_id
                .as_deref()
                .map(content_id)
                .filter(|s| !s.is_empty()),
        });
    }
    Ok(out)
}

/// The recipient rules of a send: at least one `to`, at most
/// [`MAX_RECIPIENTS_PER_SEND`] in all. Shared with the non-production redirect,
/// which checks the recipients the call NAMED, so staging refuses what
/// production would.
fn check_recipients(to: &[String], cc: &[String], bcc: &[String]) -> Result<(), String> {
    if to.is_empty() {
        return Err("InvalidEmailPayload: at least one `to` recipient is required".to_string());
    }
    let total = to.len() + cc.len() + bcc.len();
    if total > MAX_RECIPIENTS_PER_SEND {
        return Err(format!(
            "TooManyRecipients: {total} recipients exceeds the per-send limit of \
             {MAX_RECIPIENTS_PER_SEND}"
        ));
    }
    Ok(())
}

/// Normalize a `string | string[]` recipient field: trim each address and drop
/// the blanks a template can easily interpolate (`[user.email]` where the row
/// had none).
fn addresses(field: Option<OneOrMany>) -> Vec<String> {
    field
        .map(OneOrMany::into_vec)
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
        .collect()
}

/// Standard-base64 decode (accepts the common unpadded form too).
fn base64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(s))
}

/// Header-safe `Content-ID`: the same header-injection defense as
/// [`attachment_filename`], plus the delimiters that make up the token itself.
///
/// This value is emitted as `Content-ID: <…>` in MIME **we** now write, so it
/// is author-controlled input into a header this process owns. Under the old
/// `Content.Simple` path SES assembled the header and this was AWS's problem;
/// composing in-process moved that responsibility here.
///
/// Unlike a filename this is an **allowlist**, because a Content-ID is a token
/// (RFC 2045 §7 defers to RFC 5322 `msg-id`), not free text. Stripping only
/// CR/LF would stop header injection but still let
/// `contentId: "logo\r\nX-Injected: 1"` become
/// `Content-ID: <logoX-Injected: 1>` — unresolvable by any `cid:` reference and
/// alarming to read in a bug report. Real-world ids (`logo`,
/// `image001.png@01D9`, `ii_abc123`) fit comfortably in this set.
///
/// Angle brackets are dropped along with everything else: lettre adds its own,
/// so a caller-supplied pair would emit `Content-ID: <<logo>>`.
/// Also reached from `emails::mime`, which must run the **filename** through
/// this when an inline part supplies no `contentId` — `attachment_filename`
/// allows spaces, `<`, `>`, `:` and non-ASCII, none of which belong in a
/// msg-id token.
pub(super) fn content_id(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | '+'))
        .take(200)
        .collect()
}

/// Header-safe attachment filename: drop CR/LF and quote characters (MIME
/// header-injection defense) and any path separators, then bound the length.
///
/// Bounded by **characters**, not bytes. `String::truncate` panics when the
/// byte index is not a char boundary, and this keeps non-ASCII deliberately
/// (lettre RFC-2231-encodes it), so a 100-emoji filename — 300 bytes, every
/// boundary a multiple of 3 — would have panicked inside the function host on
/// `truncate(200)`.
fn attachment_filename(name: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    base.chars()
        .filter(|c| !c.is_control() && !matches!(c, '"' | '\r' | '\n'))
        .collect::<String>()
        // Trimmed on BOTH sides of the cap. After, because a >200-char name
        // whose 200th character is a space would otherwise leave that space
        // inside `filename="…"`. Before, because 200 leading spaces followed
        // by a real name would otherwise truncate to pure whitespace and then
        // to nothing, turning an absurd-but-recoverable name into a rejected
        // send.
        .trim()
        .chars()
        .take(200)
        .collect::<String>()
        .trim()
        .to_string()
}

/// Render the attachment list for the local preview (which never sends, so the
/// parts would otherwise be invisible). Renders each part as a mail-client-style
/// chip (icon · name · size · type · disposition) under a clear "preview only"
/// banner, so a developer sees exactly what WOULD have been attached.
fn attachment_manifest(attachments: &[ValidatedAttachment], ext: &str) -> String {
    let n = attachments.len();
    let plural = if n == 1 { "" } else { "s" };
    if ext == "html" {
        let chips: String = attachments
            .iter()
            .map(|a| {
                let ct = a
                    .content_type
                    .as_deref()
                    .unwrap_or("application/octet-stream");
                let disposition = if a.inline { "inline" } else { "attachment" };
                format!(
                    "<div style=\"display:flex;align-items:center;gap:10px;border:1px solid #e5e7eb;\
                     border-radius:8px;background:#f9fafb;padding:8px 12px;margin-top:6px;max-width:380px\">\
                       <div style=\"font-size:22px;line-height:1\">{icon}</div>\
                       <div style=\"min-width:0\">\
                         <div style=\"font-weight:600;color:#111827;white-space:nowrap;overflow:hidden;\
                          text-overflow:ellipsis\">{name}</div>\
                         <div style=\"color:#6b7280;font-size:12px\">{size} &middot; {label} &middot; {disposition}</div>\
                       </div>\
                     </div>",
                    icon = file_emoji(ct, &a.filename),
                    name = html_escape(&a.filename),
                    size = human_size(a.bytes.len()),
                    label = html_escape(&type_label(ct)),
                )
            })
            .collect();
        format!(
            "<hr style=\"border:none;border-top:1px solid #e5e7eb;margin:16px 0\">\
             <section style=\"font:13px/1.5 -apple-system,system-ui,sans-serif;color:#374151\">\
               <div style=\"font-weight:600;color:#111827\">\
                 \u{1F4CE} {n} attachment{plural} \
                 <span style=\"font-weight:400;color:#9ca3af\">— preview only, not sent</span>\
               </div>{chips}\
             </section>"
        )
    } else {
        let lines: String = attachments
            .iter()
            .map(|a| {
                let ct = a
                    .content_type
                    .as_deref()
                    .unwrap_or("application/octet-stream");
                let disposition = if a.inline { "inline" } else { "attachment" };
                format!(
                    "  • {} ({}, {}, {})",
                    a.filename,
                    human_size(a.bytes.len()),
                    type_label(ct),
                    disposition
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!("\n\n--- {n} attachment{plural} (preview only, not sent) ---\n{lines}\n")
    }
}

/// Bytes as a compact human size ("2035" → "2.0 KB").
fn human_size(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let b = bytes as f64;
    if b < KB {
        format!("{bytes} B")
    } else if b < MB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{:.1} MB", b / MB)
    }
}

/// A short human label for a MIME type ("text/csv" → "CSV").
fn type_label(content_type: &str) -> String {
    let base = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim();
    match base {
        "text/csv" => "CSV".to_string(),
        "application/pdf" => "PDF".to_string(),
        "application/json" => "JSON".to_string(),
        "text/plain" => "Text".to_string(),
        "text/html" => "HTML".to_string(),
        "image/png" => "PNG".to_string(),
        "image/jpeg" => "JPEG".to_string(),
        "image/gif" => "GIF".to_string(),
        "application/zip" => "ZIP".to_string(),
        other => other.rsplit('/').next().unwrap_or(other).to_uppercase(),
    }
}

/// An emoji for the preview chip, keyed off the content type (then the extension).
fn file_emoji(content_type: &str, filename: &str) -> &'static str {
    let base = content_type.split(';').next().unwrap_or("").trim();
    if base.starts_with("image/") {
        return "🖼️";
    }
    match base {
        "application/pdf" => "📄",
        "text/csv"
        | "application/vnd.ms-excel"
        | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "📊",
        "application/zip" | "application/gzip" => "📦",
        "text/plain" | "text/html" | "application/json" => "📄",
        _ => match filename
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "csv" | "xlsx" | "xls" => "📊",
            "pdf" => "📄",
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg" => "🖼️",
            "zip" | "gz" | "tar" => "📦",
            "txt" | "json" | "html" | "md" => "📄",
            _ => "📎",
        },
    }
}

/// Minimal HTML escaping for the preview manifest (filenames are author-supplied).
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The app name with header-unsafe characters removed, and **not** quoted.
///
/// This is what `emails::mime` hands to lettre: `Mailbox` takes an unencoded
/// display name and does its own RFC-5322 quoting and RFC-2047 encoding, so
/// passing the already-quoted [`display_name`] would double-quote it and ship
/// visible `""` to the recipient. The sanitising half is shared because it is a
/// header-injection defense either way.
pub(super) fn sanitized_display_name(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control() && !matches!(c, '<' | '>'))
        .collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}

/// Directory for locally-rendered email previews — one dedicated subdir of the
/// system temp dir (so previews never litter temp), created on demand and kept
/// small by `prune_old_previews`.
fn preview_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("oxy-email-previews");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Age at which a stale preview file becomes eligible for cleanup.
const PREVIEW_TTL: std::time::Duration = std::time::Duration::from_secs(3600);

/// Best-effort cleanup: delete preview files older than `PREVIEW_TTL` so the dir
/// stays small. Runs before each write; the just-written file survives until a
/// later send ages it out.
fn prune_old_previews(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let aged_out = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|mtime| now.duration_since(mtime).ok())
            .is_some_and(|age| age > PREVIEW_TTL);
        if aged_out {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Process-wide SES client, built once. The client holds a credentials
/// *provider* (not static creds), so it refreshes creds per call — caching it
/// avoids re-resolving the full AWS config (and its IMDS round-trips) on every
/// send, which matters under the per-invocation send cap. Region is a process
/// constant (env), so the first caller's region wins.
static SES_CLIENT: tokio::sync::OnceCell<aws_sdk_sesv2::Client> =
    tokio::sync::OnceCell::const_new();

async fn ses_client(region: Option<&str>) -> &'static aws_sdk_sesv2::Client {
    SES_CLIENT
        .get_or_init(|| async move {
            let mut loader = aws_config::from_env();
            if let Some(r) = region {
                loader = loader.region(aws_config::Region::new(r.to_string()));
            }
            aws_sdk_sesv2::Client::new(&loader.load().await)
        })
        .await
}

/// Flatten an error and its source chain into one string. AWS `SdkError`'s own
/// Display is terse ("service error"); the useful cause — unverified sender,
/// missing credentials, throttling — lives in the source chain.
fn error_detail(e: &impl std::error::Error) -> String {
    let mut out = e.to_string();
    let mut src = e.source();
    while let Some(s) = src {
        out.push_str(": ");
        out.push_str(&s.to_string());
        src = s.source();
    }
    out
}

/// Best-effort mapping of an SES send failure onto the typed error taxonomy,
/// with an actionable hint. (v1: the label rides in the message; distinct JS
/// `error.name` values are a follow-up that needs a per-op `__oxyError` name.)
fn classify_ses_error(msg: String) -> String {
    let lower = msg.to_lowercase();
    if lower.contains("not verified") {
        format!(
            "SenderRejected: {msg} — the `from` must be a verified SES identity. Set \
             OXY_APP_EMAIL_FROM to a verified sender, or OXY_APP_EMAIL_LOCAL_TEST=1 to preview locally."
        )
    } else if lower.contains("security token")
        || lower.contains("credential")
        || lower.contains("dispatch failure")
    {
        format!(
            "EmailSendFailed: {msg} — AWS credentials/region not resolved on the server. \
             Configure the AWS env, or set OXY_APP_EMAIL_LOCAL_TEST=1 to preview locally without SES."
        )
    } else if lower.contains("reject") {
        format!("SenderRejected: {msg}")
    } else if lower.contains("suspend") || lower.contains("paused") || lower.contains("throttl") {
        format!("RateLimitExceeded: {msg}")
    } else if lower.contains("limit") || lower.contains("quota") {
        format!("DailyLimitExceeded: {msg}")
    } else {
        format!("EmailSendFailed: {msg}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emailer() -> AppEmailer {
        AppEmailer {
            app_name: "Acme Reports".to_string(),
            from_mailbox: "noreply@oxygen-hq.com".to_string(),
            aws_region: None,
            local_test: true,
        }
    }

    fn parse(json: serde_json::Value) -> EmailSendInput {
        serde_json::from_value(json).expect("valid input")
    }

    /// The smallest payload `validate` accepts.
    fn basic() -> serde_json::Value {
        serde_json::json!({ "to": "a@b.com", "subject": "hi", "text": "yo" })
    }

    /// The app name reaches the composed `From`. Quoting and RFC-2047 are
    /// lettre's job now (asserted in `emails::mime`); this only pins that the
    /// name is carried through and sanitized on the way.
    #[test]
    fn app_name_reaches_the_composed_from_header() {
        let mime = String::from_utf8(
            emailer()
                .compose(&emailer().validate(parse(basic())).expect("valid"))
                .expect("composes"),
        )
        .unwrap();
        // lettre quotes a multi-word display-name. Both spellings are legal
        // RFC 5322 and clients strip the quotes on display, so assert the name
        // and mailbox rather than pinning lettre's choice.
        let from = mime.lines().find(|l| l.starts_with("From: ")).unwrap();
        assert!(from.contains("Acme Reports"), "{from}");
        assert!(from.ends_with("<noreply@oxygen-hq.com>"), "{from}");
    }

    #[test]
    fn a_blank_app_name_composes_a_bare_mailbox() {
        let mut e = emailer();
        e.app_name = "  ".to_string();
        let mime = String::from_utf8(
            e.compose(&e.validate(parse(basic())).expect("valid"))
                .unwrap(),
        )
        .unwrap();
        assert!(mime.contains("From: noreply@oxygen-hq.com"), "{mime}");
    }

    #[test]
    fn sanitized_display_name_drops_injection_characters() {
        // CR/LF (header injection) and <> are removed before the name ever
        // reaches lettre. Quoting is deliberately NOT done here — doing it in
        // two places is what shipped a pre-quoted name into an encoded-word.
        let n = sanitized_display_name("Ac<me>\r\n\"X").unwrap();
        assert!(
            !n.contains('<') && !n.contains('>') && !n.contains('\r') && !n.contains('\n'),
            "{n}"
        );
    }

    #[test]
    fn sanitized_display_name_empty_after_cleaning_is_none() {
        assert!(sanitized_display_name("  <>  ").is_none());
    }

    /// The name is author-adjacent (it is the app's title), so a CRLF in it
    /// must not be able to add a header to the composed message.
    #[test]
    fn an_app_name_cannot_inject_a_header() {
        let mut e = emailer();
        e.app_name = "Acme\r\nX-Injected: 1".to_string();
        let mime = String::from_utf8(
            e.compose(&e.validate(parse(basic())).expect("valid"))
                .unwrap(),
        )
        .unwrap();
        assert!(!mime.contains("X-Injected:"), "{mime}");
    }

    #[test]
    fn classify_ses_error_labels_the_cause() {
        assert!(
            classify_ses_error("Email address is not verified".into())
                .starts_with("SenderRejected")
        );
        // Credential/token errors get the "not resolved" config hint.
        assert!(
            classify_ses_error("The security token is expired".into())
                .contains("credentials/region not resolved")
        );
        assert!(classify_ses_error("some opaque failure".into()).starts_with("EmailSendFailed"));
    }

    #[test]
    fn rejects_caller_supplied_from() {
        let err = emailer()
            .validate(parse(serde_json::json!({
                "to": "a@b.com", "subject": "hi", "text": "yo", "from": "spoof@evil.com"
            })))
            .unwrap_err();
        assert!(err.contains("`from` is not settable"), "{err}");
    }

    #[test]
    fn requires_a_recipient() {
        let err = emailer()
            .validate(parse(serde_json::json!({ "subject": "hi", "text": "yo" })))
            .unwrap_err();
        assert!(err.contains("`to` recipient is required"), "{err}");
    }

    #[test]
    fn requires_subject() {
        let err = emailer()
            .validate(parse(serde_json::json!({ "to": "a@b.com", "text": "yo" })))
            .unwrap_err();
        assert!(err.contains("`subject` is required"), "{err}");
    }

    #[test]
    fn requires_a_body() {
        let err = emailer()
            .validate(parse(
                serde_json::json!({ "to": "a@b.com", "subject": "hi" }),
            ))
            .unwrap_err();
        assert!(err.contains("provide `html`"), "{err}");
    }

    #[test]
    fn enforces_recipient_cap() {
        let many: Vec<String> = (0..60).map(|i| format!("u{i}@b.com")).collect();
        let err = emailer()
            .validate(parse(serde_json::json!({
                "to": many, "subject": "hi", "text": "yo"
            })))
            .unwrap_err();
        assert!(err.starts_with("TooManyRecipients"), "{err}");
    }

    #[test]
    fn accepts_string_or_array_recipients() {
        let one = emailer()
            .validate(parse(serde_json::json!({
                "to": "a@b.com", "subject": "hi", "text": "yo"
            })))
            .unwrap();
        assert_eq!(one.to, vec!["a@b.com".to_string()]);
        let many = emailer()
            .validate(parse(serde_json::json!({
                "to": ["a@b.com", "c@d.com"], "cc": "e@f.com", "subject": "hi", "html": "<b>x</b>"
            })))
            .unwrap();
        assert_eq!(many.to.len(), 2);
        assert_eq!(many.cc, vec!["e@f.com".to_string()]);
    }

    #[test]
    fn rejects_overlong_idempotency_key() {
        let err = emailer()
            .validate(parse(serde_json::json!({
                "to": "a@b.com", "subject": "hi", "text": "yo",
                "idempotencyKey": "x".repeat(300)
            })))
            .unwrap_err();
        assert!(err.contains("idempotencyKey"), "{err}");
    }

    // ── Attachments ──────────────────────────────────────────────────────────

    /// `b64("hello")` == `aGVsbG8=`.
    fn with_attachments(attachments: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "to": "a@b.com", "subject": "hi", "text": "yo", "attachments": attachments
        })
    }

    #[test]
    fn decodes_attachment_and_defaults_disposition() {
        let msg = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                { "filename": "report.pdf", "content": "aGVsbG8=", "contentType": "application/pdf" }
            ]))))
            .expect("valid");
        assert_eq!(msg.attachments.len(), 1);
        let a = &msg.attachments[0];
        assert_eq!(a.filename, "report.pdf");
        assert_eq!(a.bytes, b"hello");
        assert_eq!(a.content_type.as_deref(), Some("application/pdf"));
        assert!(!a.inline, "attachment disposition is the default");
        // Composes into a real MIME part (structure asserted in emails::mime).
        assert!(super::super::mime::build_mime(Some("App"), "a@oxy.tech", &msg).is_ok());
    }

    #[test]
    fn accepts_inline_attachment_with_content_id() {
        let msg = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                { "filename": "logo.png", "content": "aGVsbG8=", "inline": true, "contentId": "logo" }
            ]))))
            .expect("valid");
        assert!(msg.attachments[0].inline);
        assert_eq!(msg.attachments[0].content_id.as_deref(), Some("logo"));
        assert!(super::super::mime::build_mime(Some("App"), "a@oxy.tech", &msg).is_ok());
    }

    /// The end of the pipeline: what SES actually receives.
    ///
    /// Now that Oxy composes the MIME itself, this asserts the finished message
    /// really rides in `Content.Raw` and that the envelope still carries every
    /// recipient — the two halves that only exist once the request is
    /// serialized. Part-level structure is asserted offline in `emails::mime`.
    ///
    /// Uses a capturing HTTP client: no network, no credentials, no emulator.
    #[tokio::test]
    async fn ses_request_carries_our_mime_as_raw_content() {
        use base64::Engine as _;

        let (http_client, captured) = aws_smithy_http_client::test_util::capture_request(None);
        let conf = aws_sdk_sesv2::Config::builder()
            .region(aws_sdk_sesv2::config::Region::new("us-east-1"))
            .credentials_provider(aws_credential_types::Credentials::for_tests())
            .http_client(http_client)
            .behavior_version(aws_sdk_sesv2::config::BehaviorVersion::latest())
            .build();
        let client = aws_sdk_sesv2::Client::from_conf(conf);

        let jpeg: &[u8] = &[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x0D, 0x0A, 0x80, 0x00];
        let mut input = with_attachments(serde_json::json!([
            {
                "filename": "IMG_20260330_155601_2.jpg",
                "content": base64::engine::general_purpose::STANDARD.encode(jpeg),
                "contentType": "image/jpeg"
            }
        ]));
        input["bcc"] = serde_json::json!("blind@oxy.tech");
        let msg = emailer().validate(parse(input)).expect("valid");

        // The canned response isn't a real SES reply, so the call errors after
        // the request is built — which is all this test cares about.
        let _ = emailer()
            .build_send_email(&client, &msg, emailer().compose(&msg).expect("composes"))
            .expect("request must build")
            .send()
            .await;

        let req = captured.expect_request();
        let body: serde_json::Value =
            serde_json::from_slice(req.body().bytes().expect("in-memory body")).unwrap();

        assert!(
            body["Content"]["Simple"].is_null(),
            "the simple path let SES choose the transfer encoding; it must be gone"
        );
        let raw = body["Content"]["Raw"]["Data"]
            .as_str()
            .expect("Raw.Data is a base64 Blob on the JSON wire");
        let mime = String::from_utf8(
            base64::engine::general_purpose::STANDARD
                .decode(raw)
                .expect("Blob is base64"),
        )
        .expect("our MIME is ASCII");

        // Matched as one contiguous run so the encoding is pinned to the JPEG's
        // own part. 7bit on the ASCII text body beside it is legal and expected.
        assert!(
            mime.contains("Content-Type: image/jpeg\r\nContent-Transfer-Encoding: base64"),
            "the binary part must declare base64:\n{mime}"
        );

        // The envelope still has to name everyone, including the blind copy —
        // that is what makes omitting Bcc from the headers safe.
        assert_eq!(body["Destination"]["ToAddresses"][0], "a@b.com");
        assert_eq!(body["Destination"]["BccAddresses"][0], "blind@oxy.tech");
        assert!(
            !mime.contains("blind@oxy.tech"),
            "a blind recipient must not be disclosed in the headers:\n{mime}"
        );
    }

    #[test]
    fn attaches_utf8_text_without_base64() {
        // The isolate has no base64 encoder, so a function that generates a
        // report must be able to attach the text it already holds.
        let msg = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                {
                    "filename": "report.csv",
                    "content": "name,total\nCafé,3\n",
                    "encoding": "utf8",
                    "contentType": "text/csv"
                }
            ]))))
            .expect("valid");
        assert_eq!(msg.attachments[0].bytes, "name,total\nCafé,3\n".as_bytes());
        // Non-ASCII survives byte-exact — the case `btoa` cannot express at all.
        assert!(super::super::mime::build_mime(Some("App"), "a@oxy.tech", &msg).is_ok());
    }

    #[test]
    fn utf8_encoding_preserves_whitespace_verbatim() {
        // base64 strips whitespace before decoding; utf8 must NOT, or every
        // newline in an attached CSV would vanish.
        let msg = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                { "filename": "a.txt", "content": "a\n b\t", "encoding": "utf8" }
            ]))))
            .expect("valid");
        assert_eq!(msg.attachments[0].bytes, b"a\n b\t");
    }

    #[test]
    fn rejects_unknown_attachment_encoding() {
        let err = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                { "filename": "a.txt", "content": "aGVsbG8=", "encoding": "hex" }
            ]))))
            .unwrap_err();
        assert!(err.contains("unknown `encoding`"), "{err}");
    }

    #[test]
    fn attachment_encoding_defaults_to_base64() {
        let msg = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                { "filename": "a.txt", "content": "aGVsbG8=" }
            ]))))
            .expect("valid");
        assert_eq!(msg.attachments[0].bytes, b"hello", "default is base64");
    }

    #[test]
    fn tolerates_whitespace_wrapped_base64() {
        let msg = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                { "filename": "a.txt", "content": "aGVs\nbG8=\n" }
            ]))))
            .expect("valid");
        assert_eq!(msg.attachments[0].bytes, b"hello");
    }

    /// `contentId` is author-controlled and lands in a `Content-ID:` header
    /// this process now writes — under the old `Content.Simple` path SES
    /// assembled that header, so escaping it was AWS's problem. The same
    /// defense `attachment_filename` has always had applies here.
    #[test]
    fn content_id_cannot_escape_its_header() {
        let msg = emailer()
            .validate(parse(with_attachments(serde_json::json!([{
                "filename": "logo.png",
                "content": "aGVsbG8=",
                "inline": true,
                "contentId": "logo\r\nX-Injected: 1"
            }]))))
            .expect("valid");
        let cid = msg.attachments[0].content_id.as_deref().unwrap();
        assert_eq!(
            cid, "logoX-Injected1",
            "only token characters survive: {cid:?}"
        );

        // And it must not reappear as a header once composed.
        let mime = String::from_utf8(emailer().compose(&msg).expect("composes")).unwrap();
        assert!(
            !mime.contains("X-Injected:"),
            "a caller must not be able to add a header:\n{mime}"
        );
    }

    /// lettre wraps the value in angle brackets itself, so caller-supplied ones
    /// would emit `<<logo>>` — a Content-ID no `cid:` reference can resolve.
    #[test]
    fn content_id_strips_caller_supplied_angle_brackets() {
        let msg = emailer()
            .validate(parse(with_attachments(serde_json::json!([{
                "filename": "logo.png",
                "content": "aGVsbG8=",
                "inline": true,
                "contentId": "<logo>"
            }]))))
            .expect("valid");
        assert_eq!(msg.attachments[0].content_id.as_deref(), Some("logo"));
        let mime = String::from_utf8(emailer().compose(&msg).expect("composes")).unwrap();
        assert!(mime.contains("Content-ID: <logo>"), "{mime}");
        assert!(!mime.contains("<<"), "{mime}");
    }

    /// A contentId that is nothing but delimiters sanitizes to empty, which
    /// must fall back to the filename rather than emitting `Content-ID: <>`.
    #[test]
    fn content_id_that_sanitizes_to_empty_falls_back_to_the_filename() {
        let msg = emailer()
            .validate(parse(with_attachments(serde_json::json!([{
                "filename": "logo.png",
                "content": "aGVsbG8=",
                "inline": true,
                "contentId": "<>"
            }]))))
            .expect("valid");
        assert!(msg.attachments[0].content_id.is_none());
        let mime = String::from_utf8(emailer().compose(&msg).expect("composes")).unwrap();
        assert!(mime.contains("Content-ID: <logo.png>"), "{mime}");
    }

    #[test]
    fn recipient_addresses_are_trimmed_and_blanks_dropped() {
        let msg = emailer()
            .validate(parse(serde_json::json!({
                "to": [" a@b.com ", "", "   "],
                "cc": " c@d.com",
                "replyTo": "  r@s.com  ",
                "subject": "hi",
                "text": "yo"
            })))
            .expect("valid");
        assert_eq!(msg.to, vec!["a@b.com"]);
        assert_eq!(msg.cc, vec!["c@d.com"]);
        assert_eq!(msg.reply_to.as_deref(), Some("r@s.com"));
        // And the trimmed forms must actually compose.
        assert!(emailer().compose(&msg).is_ok());
    }

    /// `String::truncate` panics when the byte index is not a char boundary,
    /// and this sanitizer deliberately keeps non-ASCII (lettre RFC-2231-encodes
    /// it). 100 coffee emoji is 300 bytes with every boundary a multiple of 3,
    /// so a byte-wise cap at 200 split a character and panicked inside the
    /// function host. Bounding by chars is the fix.
    #[test]
    fn a_long_non_ascii_filename_is_bounded_without_panicking() {
        // 300 chars / 900 bytes: byte 200 lands mid-character (900 boundaries
        // are all multiples of 3), which is precisely what used to panic.
        let name = "☕".repeat(300);
        let out = attachment_filename(&name);
        assert_eq!(out.chars().count(), 200, "bounded by chars, not bytes");
        // And it survives a real compose.
        let msg = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                { "filename": name, "content": "aGVsbG8=" }
            ]))))
            .expect("valid");
        assert!(emailer().compose(&msg).is_ok());
    }

    #[test]
    fn rejects_bad_base64_rather_than_sending_an_empty_part() {
        let err = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                { "filename": "a.txt", "content": "!!!not base64!!!" }
            ]))))
            .unwrap_err();
        assert!(err.contains("not valid base64"), "{err}");
    }

    /// Empty `content` is *valid* base64 — it decodes to zero bytes — so the
    /// malformed-input guard above never fires for it. Left unchecked it ships
    /// a well-formed MIME part with the right filename and type and an empty
    /// body: a 0-byte file the recipient's viewer reports as corrupt, with
    /// nothing anywhere in the logs. `content: someBase64 ?? ""` and a failed
    /// upstream read both land here.
    #[test]
    fn rejects_empty_content_rather_than_attaching_zero_bytes() {
        for content in ["", "   \n"] {
            let err = emailer()
                .validate(parse(with_attachments(serde_json::json!([
                    { "filename": "warehouse-card.png", "content": content }
                ]))))
                .unwrap_err();
            assert!(err.contains("empty"), "content {content:?} gave: {err}");
        }
        // utf8 has the same hole: an empty string is a legal decode too.
        let err = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                { "filename": "report.csv", "content": "", "encoding": "utf8" }
            ]))))
            .unwrap_err();
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn requires_filename_and_content() {
        let err = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                { "content": "aGVsbG8=" }
            ]))))
            .unwrap_err();
        assert!(err.contains("`filename`"), "{err}");
        let err = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                { "filename": "a.txt" }
            ]))))
            .unwrap_err();
        assert!(err.contains("`content`"), "{err}");
    }

    #[test]
    fn enforces_total_attachment_byte_cap() {
        // One attachment just over the cap (base64 of N zero bytes).
        let raw = vec![0u8; MAX_ATTACHMENT_TOTAL_BYTES + 1];
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
        let err = emailer()
            .validate(parse(with_attachments(serde_json::json!([
                { "filename": "big.bin", "content": b64 }
            ]))))
            .unwrap_err();
        assert!(err.starts_with("AttachmentTooLarge"), "{err}");
        assert!(
            err.contains("ctx.storage"),
            "should point at the link path: {err}"
        );
    }

    #[test]
    fn enforces_attachment_count_cap() {
        let many: Vec<serde_json::Value> = (0..MAX_ATTACHMENTS_PER_SEND + 1)
            .map(|i| serde_json::json!({ "filename": format!("f{i}.txt"), "content": "aGVsbG8=" }))
            .collect();
        let err = emailer()
            .validate(parse(with_attachments(serde_json::json!(many))))
            .unwrap_err();
        assert!(err.starts_with("TooManyAttachments"), "{err}");
    }

    #[test]
    fn attachment_filename_strips_paths_and_header_injection() {
        assert_eq!(attachment_filename("../../etc/passwd"), "passwd");
        assert_eq!(
            attachment_filename("a\r\nBcc: evil@x.com"),
            "aBcc: evil@x.com"
        );
        assert_eq!(attachment_filename("we\"ird.txt"), "weird.txt");
    }

    #[test]
    fn no_attachments_is_still_valid() {
        let msg = emailer()
            .validate(parse(serde_json::json!({
                "to": "a@b.com", "subject": "hi", "text": "yo"
            })))
            .expect("valid");
        assert!(msg.attachments.is_empty());
    }

    #[test]
    fn preview_manifest_helpers_are_human_readable() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(2035), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(type_label("text/csv"), "CSV");
        assert_eq!(type_label("application/pdf"), "PDF");
        assert_eq!(type_label("application/x-thing; charset=utf-8"), "X-THING");
        assert_eq!(file_emoji("text/csv", "r.csv"), "📊");
        assert_eq!(file_emoji("application/pdf", "r.pdf"), "📄");
        assert_eq!(file_emoji("image/png", "c.png"), "🖼️");
        // Falls back to the extension when the content type is generic.
        assert_eq!(file_emoji("application/octet-stream", "photo.jpg"), "🖼️");
    }

    #[test]
    fn preview_manifest_html_lists_each_attachment() {
        let atts = vec![ValidatedAttachment {
            filename: "store-report.csv".into(),
            bytes: vec![0u8; 2035],
            content_type: Some("text/csv".into()),
            inline: false,
            content_id: None,
        }];
        let html = attachment_manifest(&atts, "html");
        assert!(html.contains("1 attachment"), "{html}");
        assert!(html.contains("preview only"), "{html}");
        assert!(html.contains("store-report.csv"), "{html}");
        assert!(html.contains("2.0 KB"), "{html}");
        assert!(html.contains("CSV"), "{html}");
    }
}
