//! MIME assembly for app email (`ctx.email.send`).
//!
//! Oxy composes the whole message here and hands SES the finished bytes as
//! `Content.Raw`. The alternative — `Content.Simple`, where you pass SES a
//! subject, a body and a list of attachments and it assembles the MIME on its
//! servers — is what shipped first, and it cost two broken releases:
//!
//! SES does not infer a transfer encoding per part. With
//! `Attachment.ContentTransferEncoding` left unset it emitted
//! `Content-Transfer-Encoding: 7bit` on an `image/jpeg`. RFC 2045 §2.7 defines
//! 7bit as US-ASCII — no NUL, no 8-bit octets, no bare CR/LF — all three of
//! which appear in the first bytes of a JPEG, so the payload was stripped
//! crossing SMTP and every binary attachment arrived corrupt.
//!
//! The deeper problem was not the default; it was that **nothing on this side
//! of the network could observe it**. A test can assert what we send *to* SES,
//! never what SES emits, and no emulator helps: LocalStack reimplements the
//! MIME generation, so it would have gone green on the broken code. Owning the
//! bytes moves the contract into this process, where
//! [`tests`] asserts it byte-exact, offline, in CI.
//!
//! Nothing here picks an encoding heuristically — attachments are forced to
//! base64 (see [`attachment_part`]). That is the whole lesson of the bug.

use lettre::Address;
use lettre::message::header::{ContentTransferEncoding, ContentType};
use lettre::message::{Attachment, Body, Mailbox, Message, MultiPart, SinglePart};

use super::app_emailer::{ValidatedAttachment, ValidatedEmail};

/// Fallback part type when the author supplied none, or supplied one that is
/// not a parseable MIME type. `application/octet-stream` is the RFC 2046 §4.5.1
/// default and makes the recipient's client treat the part as opaque bytes to
/// save — never as text to render.
const DEFAULT_ATTACHMENT_CONTENT_TYPE: &str = "application/octet-stream";

/// Compose `msg` into RFC-5322 bytes ready for SES `Content.Raw`.
///
/// The sender arrives **decomposed** — `from_name` sanitized but unencoded,
/// `from_mailbox` a bare address — rather than as a preformatted
/// `Name <addr>` string. Formatting a display name only to have lettre parse
/// it back and re-encode it means two escaping implementations have to agree:
/// a quoted-string would round-trip to visible `""`, and a non-ASCII name has
/// to survive as RFC-2047. Constructing the `Mailbox` directly deletes the
/// question — lettre owns the encoding, end to end.
///
/// **Bcc is deliberately absent from the output.** Blind recipients ride SES's
/// `Destination.BccAddresses` as envelope recipients; writing them into the
/// headers is what turns a blind copy into a disclosed one.
pub(super) fn build_mime(
    from_name: Option<&str>,
    from_mailbox: &str,
    msg: &ValidatedEmail,
) -> Result<Vec<u8>, String> {
    let from_address = from_mailbox.trim().parse::<Address>().map_err(|e| {
        // NOT InvalidEmailPayload: this comes from OXY_APP_EMAIL_FROM, which a
        // function author cannot reach. Telling them to fix their payload would
        // send them chasing a server misconfiguration.
        format!(
            "EmailNotConfigured: the platform sender address '{from_mailbox}' is not a valid \
             mailbox ({e}); check OXY_APP_EMAIL_FROM"
        )
    })?;
    let mut builder =
        Message::builder().from(Mailbox::new(from_name.map(str::to_string), from_address));
    for to in &msg.to {
        builder = builder.to(mailbox(to, "to")?);
    }
    for cc in &msg.cc {
        builder = builder.cc(mailbox(cc, "cc")?);
    }
    if let Some(reply_to) = &msg.reply_to {
        builder = builder.reply_to(mailbox(reply_to, "replyTo")?);
    }
    // lettre RFC-2047-encodes the subject, so non-ASCII needs no help here.
    builder = builder.subject(msg.subject.as_str());
    // No Message-ID: SES assigns its own on send, and a second one would be a
    // conflicting identity for the same message. lettre only auto-inserts Date.

    // Two nested containers, each added only when it has something to hold:
    //
    //   multipart/mixed
    //     multipart/related      <- body + the parts the HTML references
    //       multipart/alternative (or a lone text/html | text/plain)
    //       inline part(s)
    //     downloadable part(s)
    //
    // The `related` layer is what makes `cid:` resolution well-defined
    // (RFC 2387). Putting an inline image directly in `mixed` as a sibling of
    // the body leaves the reference undefined: Gmail usually copes, but Outlook
    // and Apple Mail commonly render it as a separate download instead of in
    // place. The structure is ours now, so it should be the RFC one rather than
    // the one most clients happen to tolerate.
    let (inline, downloadable): (Vec<_>, Vec<_>) = msg.attachments.iter().partition(|a| a.inline);

    let (mut content, root_type) = body_part(msg)?;
    if !inline.is_empty() {
        let mut related = content.nest_in(MultiPart::related().build());
        // Content-IDs must be unique within the container or `cid:` is
        // ambiguous — and the fallback is a pure function of the filename, so
        // a loop attaching one `logo.png` per row would mint the same id every
        // time. De-duplicated by suffixing the position.
        let mut seen = std::collections::HashSet::with_capacity(inline.len());
        let mut renamed = Vec::new();
        for (idx, attachment) in inline.into_iter().enumerate() {
            let (cid, collision) = unique_content_id(attachment, idx, &mut seen);
            renamed.extend(collision);
            related = related.singlepart(inline_part(attachment, cid)?);
        }
        warn_about_renamed_ids(&renamed);
        set_related_root_type(&mut related, root_type);
        content = BodyPart::Multi(related);
    }
    if !downloadable.is_empty() {
        let mut mixed = content.nest_in(MultiPart::mixed().build());
        for attachment in downloadable {
            mixed = mixed.singlepart(attachment_part(attachment)?);
        }
        content = BodyPart::Multi(mixed);
    }

    let message = match content {
        BodyPart::Single(part) => builder.singlepart(part),
        BodyPart::Multi(part) => builder.multipart(part),
    }
    .map_err(|e| format!("InvalidEmailPayload: could not compose the message: {e}"))?;

    Ok(message.formatted())
}

/// The readable body, before attachments are wrapped around it.
enum BodyPart {
    Single(SinglePart),
    Multi(MultiPart),
}

impl BodyPart {
    /// Place this part inside `container`, picking the right arm for its shape.
    fn nest_in(self, container: MultiPart) -> MultiPart {
        match self {
            BodyPart::Single(part) => container.singlepart(part),
            BodyPart::Multi(part) => container.multipart(part),
        }
    }
}

/// `multipart/alternative` when both bodies are present so each client picks
/// the one it renders best, otherwise the single body we were given.
///
/// Returns the body **and** the media type a `multipart/related` container has
/// to declare as its root (RFC 2387 §3.1). They travel together so they cannot
/// disagree: deriving the type from `msg` in a second match elsewhere would
/// make the `type` parameter silently lie the moment this function's shape
/// changed, and would need its own (unreachable) arm for the no-body case.
fn body_part(msg: &ValidatedEmail) -> Result<(BodyPart, &'static str), String> {
    match (msg.html.as_deref(), msg.text.as_deref()) {
        (Some(html), Some(text)) => Ok((
            BodyPart::Multi(MultiPart::alternative_plain_html(
                text.to_owned(),
                html.to_owned(),
            )),
            "multipart/alternative",
        )),
        (Some(html), None) => Ok((
            BodyPart::Single(SinglePart::html(html.to_owned())),
            "text/html",
        )),
        (None, Some(text)) => Ok((
            BodyPart::Single(SinglePart::plain(text.to_owned())),
            "text/plain",
        )),
        // Unreachable via `validate`, which requires one of the two. Kept as an
        // error rather than a panic so a future caller can't turn it into one.
        (None, None) => {
            Err("InvalidEmailPayload: message has neither `html` nor `text` to send".to_string())
        }
    }
}

/// One attachment as a `SinglePart`, **always** base64.
///
/// The encoding is forced rather than inferred. lettre would in fact choose
/// base64 for a `Vec<u8>` on its own, but "the library picks a sensible
/// default for binary" is precisely the assumption that made SES emit 7bit,
/// and it is not worth re-learning. `Body::new_with_encoding` accepts base64
/// for any input by construction, so the error arm is unreachable in practice.
fn attachment_part(a: &ValidatedAttachment) -> Result<SinglePart, String> {
    Ok(Attachment::new(a.filename.clone()).body(attachment_body(a)?, content_type_for(a)))
}

/// An inline part, carrying the `Content-ID` the HTML addresses it by. lettre
/// adds the angle brackets.
fn inline_part(a: &ValidatedAttachment, content_id: String) -> Result<SinglePart, String> {
    Ok(
        Attachment::new_inline_with_name(content_id, a.filename.clone())
            .body(attachment_body(a)?, content_type_for(a)),
    )
}

/// The base64 body shared by both part shapes.
fn attachment_body(a: &ValidatedAttachment) -> Result<Body, String> {
    Body::new_with_encoding(a.bytes.clone(), ContentTransferEncoding::Base64).map_err(|_| {
        format!(
            "InvalidEmailPayload: attachment '{}' could not be base64-encoded",
            a.filename
        )
    })
}

/// A `Content-ID` unique within the `related` container.
///
/// The base id cannot simply be `a.filename`. That string went through
/// `attachment_filename`, which is a *filename* defense and therefore keeps
/// spaces, `<`, `>`, `:` and every non-ASCII character. A perfectly ordinary
/// `{ filename: "Café Logo.png", inline: true }` with no `contentId` would
/// then emit `Content-ID: <Café Logo.png>`: raw 8-bit bytes in a header, or an
/// encoded-word no `cid:` reference can resolve. Same allowlist, same reason as
/// the author-supplied path — this is the door beside it.
///
/// Sanitizing preserves the common `cid:report.png` reference for ASCII
/// filenames. A filename that is *all* non-token characters falls back to a
/// positional id.
///
/// Uniqueness is then enforced against `seen`: the base id is a pure function
/// of the filename, so a loop attaching one `logo.png` per row would otherwise
/// mint `<logo.png>` repeatedly and `cid:logo.png` would resolve to whichever
/// part the client happened to index first. The positional fallback collides
/// the same way (`["☕", "attachment0"]` yields `attachment0` twice), so it is
/// checked too.
fn unique_content_id(
    a: &ValidatedAttachment,
    idx: usize,
    seen: &mut std::collections::HashSet<String>,
) -> (String, Option<Renamed>) {
    let base = match &a.content_id {
        Some(id) => id.clone(),
        None => {
            let from_filename = super::app_emailer::content_id(&a.filename);
            if from_filename.is_empty() {
                format!("attachment{idx}")
            } else {
                from_filename
            }
        }
    };
    // `seen.insert` IS the check: it returns false when the id is taken. A
    // single `{base}.{idx}` is not enough on its own, because that string can
    // itself already be taken — contentIds ["logo", "logo.2", "logo"] would
    // have the third part land on "logo.2", and the filename path reaches the
    // same place since the allowlist keeps `.` and digits. Advancing the
    // counter terminates: every step tries a new suffix and the set is finite.
    let mut id = base.clone();
    let mut n = idx;
    while !seen.insert(id.clone()) {
        id = format!("{base}.{n}");
        n += 1;
    }
    let renamed = (id != base).then(|| Renamed {
        filename: a.filename.clone(),
        requested: base,
        assigned: id.clone(),
    });
    (id, renamed)
}

/// An inline part whose Content-ID had to be changed to stay unique.
struct Renamed {
    filename: String,
    requested: String,
    assigned: String,
}

/// Report inline parts whose Content-ID had to change, **once per send**.
///
/// Renaming is the only safe resolution, but it is invisible to the author:
/// their `cid:{requested}` still resolves — to the FIRST part — so the second
/// image renders as nothing with no error anywhere. Without this, "why is my
/// second image blank" is only answerable by reading the composer.
///
/// Emitted once rather than per collision because the pathological case is a
/// loop attaching one `logo.png` per row: under the 20-part cap that is up to
/// 19 renames for a single logical mistake, and 19 warnings read as 19
/// incidents.
///
/// Every DISTINCT colliding id is named, not just the first. Two independent
/// groups (`logo.png`×2 and `banner.png`×2) would otherwise report a count of
/// 2 with only `logo.png` as the example, so an author who fixed that one
/// would re-run straight into the other with nothing new to go on. Bounded by
/// the per-send attachment cap.
fn warn_about_renamed_ids(renamed: &[Renamed]) {
    if renamed.is_empty() {
        return;
    }
    let list = |mut values: Vec<&str>| {
        values.sort_unstable();
        values.dedup();
        values.join(", ")
    };
    tracing::warn!(
        count = renamed.len(),
        collisions = %list(renamed.iter().map(|r| r.requested.as_str()).collect()),
        filenames = %list(renamed.iter().map(|r| r.filename.as_str()).collect()),
        assigned = %list(renamed.iter().map(|r| r.assigned.as_str()).collect()),
        // The advice names filenames as well as `contentId`: the collision is
        // reachable with no `contentId` supplied at all (two files both called
        // logo.png, ids derived from the filename), and an author told only to
        // fix `contentId` would go looking for a field they never set.
        "duplicate inline Content-ID(s): {} part(s) were renamed to stay unique, so a \
         `cid:` reference to the original resolves to the earlier part and the later \
         one will not render; give each inline attachment a distinct `contentId`, or \
         distinct filenames when you do not set one",
        renamed.len()
    );
}

/// Add the RFC 2387 §3.1 `type` parameter to a `multipart/related` container.
///
/// "The type parameter must be specified and its value is the MIME media type
/// of the root body part." lettre's `MultiPart::related()` does not emit it —
/// its `kind()` builds the Content-Type from a fixed template — so the header
/// is rewritten here, preserving the boundary lettre already generated (it is
/// baked into the header at `kind()` time and read back out at format time).
///
/// In practice every client treats the first part as the root and renders
/// correctly without this, but that is the same "clients tolerate it" argument
/// rejected in favour of `related` over `mixed` one layer up; the structure is
/// ours, so it should be the RFC one. A parse failure leaves the container
/// exactly as lettre built it — valid, just missing an advisory parameter.
fn set_related_root_type(related: &mut MultiPart, root_type: &str) {
    let boundary = related.boundary();
    if let Ok(content_type) = ContentType::parse(&format!(
        "multipart/related; boundary=\"{boundary}\"; type=\"{root_type}\""
    )) {
        related.headers_mut().set(content_type);
    }
}

/// The author's `contentType` when it parses, else octet-stream. An
/// unparseable type is not worth failing a send over — the fallback still
/// delivers the bytes intact, which is what the recipient actually needs.
fn content_type_for(a: &ValidatedAttachment) -> ContentType {
    a.content_type
        .as_deref()
        .and_then(|ct| ContentType::parse(ct).ok())
        .unwrap_or_else(|| {
            ContentType::parse(DEFAULT_ATTACHMENT_CONTENT_TYPE)
                .expect("octet-stream is a valid MIME type")
        })
}

/// Parse one address into a lettre `Mailbox`, naming the offending field.
fn mailbox(addr: &str, field: &str) -> Result<Mailbox, String> {
    addr.parse::<Mailbox>().map_err(|e| {
        format!("InvalidEmailPayload: `{field}` address '{addr}' is not a valid mailbox: {e}")
    })
}

#[cfg(test)]
mod tests;
