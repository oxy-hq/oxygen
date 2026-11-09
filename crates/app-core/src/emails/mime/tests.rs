//! Offline assertions on the MIME `super` composes.
//!
//! Split from `mime.rs` purely for size — the module was 976 lines against
//! the ~400 guideline in `internal-docs/backend-architecture.md`, and the
//! split is mechanical: ~290 lines of code, the rest these tests.

use super::*;
use base64::Engine as _;
const FROM_NAME: &str = "Warehouse";
const FROM_ADDR: &str = "noreply@oxygen-hq.com";

/// The invariants that hold for **every** message, checked on every one
/// these tests compose — go through [`compose`] or [`compose_with`] rather
/// than calling `build_mime` directly, or the check is skipped.
///
///   - Pure ASCII. RFC 5322 headers are ASCII and every body is 7bit,
///     quoted-printable or base64, so a raw 8-bit byte anywhere means
///     something skipped its encoder.
///   - No line over 998 characters (RFC 5322 §2.1.1). SES rejects a
///     `RawMessage` that breaks it; `Content.Simple` used to satisfy this for
///     us, and composing in-process made it ours.
///   - A `MIME-Version` header (RFC 2045 §4, mandatory). lettre's builder
///     inserts it, which is precisely the kind of library default this module
///     exists to stop trusting silently — and its failure mode is total: a
///     client that does not see it treats the message as `text/plain` and
///     shows the recipient raw base64 instead of an attachment.
fn assert_message_invariants(bytes: &[u8]) {
    assert!(
        bytes.is_ascii(),
        "no raw 8-bit byte may appear in a composed message:\n{}",
        String::from_utf8_lossy(bytes)
    );
    let mime = String::from_utf8_lossy(bytes);
    assert!(
        mime.contains("MIME-Version: 1.0\r\n"),
        "every message must declare MIME-Version:\n{mime}"
    );
    if let Some(line) = mime.split("\r\n").find(|l| l.len() > 998) {
        panic!(
            "line of {} chars exceeds the RFC 5322 / SES limit of 998: {:?}",
            line.len(),
            &line[..80.min(line.len())]
        );
    }
}

/// Compose with an explicit sender, asserting the invariants above.
fn compose_with(from_name: Option<&str>, from_addr: &str, msg: &ValidatedEmail) -> Vec<u8> {
    let bytes = build_mime(from_name, from_addr, msg).expect("composes");
    assert_message_invariants(&bytes);
    bytes
}

/// Compose with the standard sender and return the message as text.
fn compose(msg: &ValidatedEmail) -> String {
    String::from_utf8(compose_with(Some(FROM_NAME), FROM_ADDR, msg))
        .expect("a MIME message is text")
}

/// Split the MIME part whose headers contain `marker` into (headers, body).
///
/// Deliberately hand-rolled rather than reaching for a parser crate: a
/// parser that shares assumptions with the writer can agree with it and
/// still be wrong together. This walks the boundaries the way a mail client
/// does — split on `--boundary`, find the part, cut at the blank line that
/// ends its headers.
///
/// Splits on *every* boundary marker in the message, so it reaches parts at
/// any nesting depth (a part inside `related` inside `mixed`).
fn part(mime: &str, marker: &str) -> (String, String) {
    let (headers, body) = mime
        .split("\r\n--")
        .find(|part| part.contains(marker))
        .unwrap_or_else(|| panic!("no part containing {marker:?} in:\n{mime}"))
        .split_once("\r\n\r\n")
        .expect("a part separates headers from body with a blank line");
    (headers.to_string(), body.trim().to_string())
}

/// Decode the base64 body of the part identified by `marker`.
fn part_bytes(mime: &str, marker: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(part(mime, marker).1.replace("\r\n", ""))
        .expect("attachment body must be valid base64")
}

fn attachment(filename: &str, bytes: Vec<u8>, content_type: Option<&str>) -> ValidatedAttachment {
    ValidatedAttachment {
        filename: filename.to_string(),
        bytes,
        content_type: content_type.map(str::to_string),
        inline: false,
        content_id: None,
    }
}

fn email(attachments: Vec<ValidatedAttachment>) -> ValidatedEmail {
    ValidatedEmail {
        subject: "Receiving Report RR-15".to_string(),
        to: vec!["nick@oxy.tech".to_string()],
        cc: vec![],
        bcc: vec![],
        reply_to: None,
        html: None,
        text: Some("report attached".to_string()),
        attachments,
    }
}

/// Every `Content-ID` value in the message, in order.
fn content_ids(mime: &str) -> Vec<String> {
    mime.match_indices("Content-ID: <")
        .map(|(i, _)| {
            let rest = &mime[i + "Content-ID: <".len()..];
            rest[..rest.find('>').expect("a Content-ID closes")].to_string()
        })
        .collect()
}

/// The bytes that broke production: a JPEG SOI marker, a NUL, a bare CR and
/// LF, and a high-bit octet — every category 7bit cannot carry.
fn jpeg_bytes() -> Vec<u8> {
    vec![
        0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x0D, 0x0A, 0x80, 0x7F, 0x00, 0xFE,
    ]
}

// ---- the regression this module exists for -----------------------------

#[test]
fn binary_attachment_is_base64_and_survives_byte_exact() {
    let bytes = jpeg_bytes();
    let mime = compose(&email(vec![attachment(
        "IMG_20260330_155601_2.jpg",
        bytes.clone(),
        Some("image/jpeg"),
    )]));

    // Scoped to the attachment's own headers on purpose: 7bit is perfectly
    // legal on the ASCII text body beside it, so a whole-message assertion
    // would fail on correct output and teach nothing.
    let (headers, _) = part(&mime, "IMG_20260330_155601_2.jpg");
    assert!(
        headers.contains("Content-Type: image/jpeg"),
        "declared type is preserved:\n{headers}"
    );
    assert!(
        headers.contains("Content-Transfer-Encoding: base64"),
        "binary MUST NOT go out as 7bit:\n{headers}"
    );
    assert!(
        !headers.contains("7bit"),
        "the part carrying 8-bit octets may never claim 7bit:\n{headers}"
    );

    assert_eq!(
        part_bytes(&mime, "IMG_20260330_155601_2.jpg"),
        bytes,
        "the attachment must round-trip byte-exact"
    );
}

/// Every format a custom app realistically attaches, in one sweep.
///
/// Each fixture is that format's real magic number followed by bytes chosen
/// to be hostile to a text transfer encoding — NUL, bare CR/LF, and octets
/// above 0x7F. The `7bit` bug corrupted all of these identically; a
/// per-format case means a future encoding change cannot quietly break one
/// container while the JPEG case stays green.
#[test]
fn every_common_attachment_format_survives_byte_exact() {
    let tail = [0x00u8, 0x0D, 0x0A, 0x80, 0xFF, 0x7F, 0x1A];
    let formats: &[(&str, &str, &[u8])] = &[
        ("report.pdf", "application/pdf", b"%PDF-1.7"),
        (
            "chart.png",
            "image/png",
            &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A],
        ),
        ("photo.jpg", "image/jpeg", &[0xFF, 0xD8, 0xFF, 0xE0]),
        ("scan.gif", "image/gif", b"GIF89a"),
        ("logo.webp", "image/webp", b"RIFF\0\0\0\0WEBP"),
        ("archive.zip", "application/zip", &[0x50, 0x4B, 0x03, 0x04]),
        ("bundle.gz", "application/gzip", &[0x1F, 0x8B, 0x08]),
        (
            "sheet.xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            &[0x50, 0x4B, 0x03, 0x04],
        ),
        (
            "deck.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            &[0x50, 0x4B, 0x03, 0x04],
        ),
        (
            "legacy.xls",
            "application/vnd.ms-excel",
            &[0xD0, 0xCF, 0x11, 0xE0],
        ),
        ("data.parquet", "application/octet-stream", b"PAR1"),
        ("db.sqlite", "application/vnd.sqlite3", b"SQLite format 3\0"),
        (
            "clip.mp4",
            "video/mp4",
            &[0x00, 0x00, 0x00, 0x18, 0x66, 0x74, 0x79, 0x70],
        ),
        (
            "note.ico",
            "image/vnd.microsoft.icon",
            &[0x00, 0x00, 0x01, 0x00],
        ),
        // Text-ish formats: these would have survived 7bit, which is exactly
        // why the original bug hid. Pinned so a future "optimization" that
        // reintroduces per-type encoding choice has to break them too.
        ("export.csv", "text/csv", "name,total\nCafé,3\n".as_bytes()),
        (
            "payload.json",
            "application/json",
            "{\"k\":\"vä\"}".as_bytes(),
        ),
        ("page.html", "text/html", b"<p>hi</p>"),
        ("notes.txt", "text/plain", b"plain"),
        (
            "feed.xml",
            "application/xml",
            b"<?xml version=\"1.0\"?><a/>",
        ),
        (
            "cal.ics",
            "text/calendar",
            b"BEGIN:VCALENDAR\r\nEND:VCALENDAR",
        ),
        ("font.woff2", "font/woff2", b"wOF2"),
        (
            "vector.svg",
            "image/svg+xml",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
        ),
    ];

    for (filename, content_type, magic) in formats {
        let mut bytes = magic.to_vec();
        bytes.extend_from_slice(&tail);

        let mime = compose(&email(vec![attachment(
            filename,
            bytes.clone(),
            Some(content_type),
        )]));
        let (headers, _) = part(&mime, filename);

        assert!(
            headers.contains(&format!("Content-Type: {content_type}")),
            "{filename}: declared type must survive:\n{headers}"
        );
        assert!(
            headers.contains("Content-Transfer-Encoding: base64"),
            "{filename}: must be base64, whatever the type says:\n{headers}"
        );
        assert_eq!(
            part_bytes(&mime, filename),
            bytes,
            "{filename}: must round-trip byte-exact"
        );
    }
}

/// Sizes that cross the base64 line-wrapping and 3-byte-group boundaries,
/// over the full 0..=255 octet range. Catches an off-by-one in chunking or
/// padding that a short fixture would sail past.
#[test]
fn attachments_round_trip_across_size_and_padding_boundaries() {
    for len in [1usize, 2, 3, 4, 57, 58, 59, 76, 77, 1024, 8192, 8193] {
        let bytes: Vec<u8> = (0..len).map(|i| (i % 256) as u8).collect();
        let mime = compose(&email(vec![attachment("blob.bin", bytes.clone(), None)]));
        assert_eq!(
            part_bytes(&mime, "blob.bin"),
            bytes,
            "a {len}-byte attachment must round-trip byte-exact"
        );
    }
}

/// A text attachment could always survive 7bit, which is exactly why the
/// original CSV-based test passed while binary was broken. Pin it anyway:
/// base64 is correct for text too, and non-ASCII must stay byte-exact.
#[test]
fn utf8_text_attachment_round_trips_with_accents() {
    let csv = "name,total\nCafé,3\n";
    let mime = compose(&email(vec![attachment(
        "report.csv",
        csv.as_bytes().to_vec(),
        Some("text/csv"),
    )]));
    assert_eq!(
        part_bytes(&mime, "report.csv"),
        csv.as_bytes(),
        "accents survive byte-exact"
    );
}

// ---- headers -----------------------------------------------------------

#[test]
fn attachment_carries_filename_and_attachment_disposition() {
    let mime = compose(&email(vec![attachment(
        "report.csv",
        b"a,b\n".to_vec(),
        Some("text/csv"),
    )]));
    assert!(
        mime.contains(r#"Content-Disposition: attachment; filename="report.csv""#),
        "{mime}"
    );
}

/// A non-ASCII filename cannot go into a header raw — RFC 5322 headers are
/// ASCII, so it needs RFC 2231 (`filename*=utf-8''…`). Asserting the whole
/// message is ASCII is the check that actually bites: `String::from_utf8`
/// happily accepts raw 8-bit UTF-8 header bytes, so the assertion style used
/// elsewhere in this file would not have caught it.
#[test]
fn non_ascii_attachment_filename_is_rfc2231_encoded() {
    let mime = compose(&email(vec![attachment(
        "Café Reporte ñ.csv",
        b"a,b\n".to_vec(),
        Some("text/csv"),
    )]));
    // ASCII is asserted by the helper; what is specific here is HOW the
    // non-ASCII name survives.
    assert!(
        mime.contains("filename*=utf-8''") || mime.contains("filename*0*=utf-8''"),
        "a non-ASCII filename must be RFC-2231 encoded:\n{mime}"
    );
}

/// Non-ASCII in the subject has to be RFC-2047 encoded or the header is
/// illegal — and a raw 8-bit subject is the other half of the 7bit trap.
#[test]
fn non_ascii_subject_is_rfc2047_encoded() {
    let mut msg = email(vec![]);
    msg.subject = "Café — Aug 4".to_string();
    let mime = compose(&msg);
    assert!(mime.contains("Subject: =?utf-8?"), "{mime}");
    assert!(
        !mime.contains("Café"),
        "raw non-ASCII must not reach a header:\n{mime}"
    );
}

/// The whole message stays ASCII even when every author-controlled string
/// is non-ASCII at once. This is the invariant that makes the message legal
/// over a 7-bit transport, and it is cheaper to assert than to enumerate
/// every header that might carry text.
#[test]
fn a_fully_non_ascii_message_still_emits_pure_ascii() {
    let mut msg = email(vec![attachment(
        "Ünicode Café.csv",
        "Café,3\n".as_bytes().to_vec(),
        Some("text/csv"),
    )]);
    msg.subject = "Rapport café — août".to_string();
    msg.text = Some("Café — naïve résumé ☕\n".to_string());
    msg.html = Some("<p>Café — naïve résumé ☕</p>".to_string());
    let mime = String::from_utf8(compose_with(Some("Café Reports ☕"), FROM_ADDR, &msg)).unwrap();
    // The helper asserts the message is ASCII. These say HOW each distinct
    // non-ASCII surface survived, so the test still fails loudly if the helper
    // is ever loosened — it is the richest fixture in the file and should not
    // be the one test that silently stops checking anything.
    // RFC 2047 encodes only the words that NEED it, so the encoded-word can
    // sit mid-header ("Subject: Rapport =?utf-8?b?...?="). Match within the
    // header rather than assuming it starts one.
    let header = |name: &str| {
        mime.lines()
            .find(|l| l.starts_with(name))
            .unwrap_or_else(|| panic!("no {name} header:\n{mime}"))
    };
    assert!(header("Subject: ").contains("=?utf-8?"), "subject:\n{mime}");
    assert!(
        header("From: ").contains("=?utf-8?"),
        "sender name:\n{mime}"
    );
    assert!(
        mime.contains("filename*=utf-8\'\'") || mime.contains("filename*0*=utf-8\'\'"),
        "attachment filename:\n{mime}"
    );
}

// ---- the sender --------------------------------------------------------

#[test]
fn display_name_survives_into_the_from_header() {
    let mime = compose(&email(vec![]));
    assert!(
        mime.contains("From: Warehouse <noreply@oxygen-hq.com>"),
        "{mime}"
    );
}

/// A name with an RFC-5322 special must reach the recipient as itself,
/// encoded exactly once. lettre resolves this with an RFC-2047 encoded-word
/// rather than a quoted-string — both are legal — so the assertion decodes
/// the header rather than pinning one spelling.
///
/// This is the case the old round-trip could have shipped wrong: passing
/// the *pre-quoted* `"Acme, Inc."` would have encoded the quotes as part of
/// the name, and the recipient would see them.
#[test]
fn a_sender_name_with_specials_is_encoded_exactly_once() {
    let bytes = compose_with(Some("Acme, Inc."), FROM_ADDR, &email(vec![]));
    let mime = String::from_utf8(bytes).unwrap();
    let from = mime
        .lines()
        .find(|l| l.starts_with("From: "))
        .expect("a From header");
    assert!(from.ends_with(&format!("<{FROM_ADDR}>")), "{from}");

    let decoded = from
        .split("=?utf-8?b?")
        .nth(1)
        .and_then(|w| w.split("?=").next())
        .map(|b64| {
            String::from_utf8(
                base64::engine::general_purpose::STANDARD
                    .decode(b64)
                    .expect("encoded-word payload is base64"),
            )
            .expect("utf-8")
        })
        // If lettre ever switches to a quoted-string, accept that spelling
        // too — what matters is the name, not the encoding chosen.
        .unwrap_or_else(|| from["From: ".len()..].trim().to_string());

    assert!(
        decoded.starts_with("Acme, Inc."),
        "the name must survive encoding exactly once, got {decoded:?} from {from:?}"
    );
    assert!(
        !decoded.contains('"'),
        "a pre-quoted name would surface its quotes to the recipient: {decoded:?}"
    );
}

#[test]
fn a_non_ascii_sender_name_is_rfc2047_encoded() {
    let bytes = compose_with(Some("Café Reports"), FROM_ADDR, &email(vec![]));
    let mime = String::from_utf8(bytes).unwrap();
    assert!(mime.contains("From: =?utf-8?"), "{mime}");
}

#[test]
fn a_sender_with_no_name_emits_a_bare_mailbox() {
    let bytes = compose_with(None, FROM_ADDR, &email(vec![]));
    let mime = String::from_utf8(bytes).unwrap();
    assert!(mime.contains("From: noreply@oxygen-hq.com"), "{mime}");
}

/// `OXY_APP_EMAIL_FROM` is server configuration a function author cannot
/// reach, so its failure must not be labelled as their payload's fault.
#[test]
fn a_misconfigured_platform_sender_is_not_blamed_on_the_payload() {
    let err = build_mime(Some("X"), "not-a-mailbox", &email(vec![])).unwrap_err();
    assert!(err.starts_with("EmailNotConfigured"), "{err}");
    assert!(err.contains("OXY_APP_EMAIL_FROM"), "{err}");
}

#[test]
fn a_malformed_recipient_is_an_error_not_a_panic() {
    let mut msg = email(vec![]);
    msg.to = vec!["not-an-address".to_string()];
    let err = build_mime(Some(FROM_NAME), FROM_ADDR, &msg).unwrap_err();
    assert!(err.starts_with("InvalidEmailPayload"), "{err}");
    assert!(err.contains("`to`"), "{err}");
}

// ---- structure ---------------------------------------------------------

#[test]
fn inline_attachment_gets_a_content_id_the_html_can_address() {
    let mut a = attachment("logo.png", vec![0x89, 0x50, 0x4E, 0x47], Some("image/png"));
    a.inline = true;
    a.content_id = Some("logo".to_string());
    let mime = compose(&email(vec![a]));
    assert!(mime.contains("Content-ID: <logo>"), "{mime}");
    assert!(mime.contains("Content-Disposition: inline"), "{mime}");
}

#[test]
fn inline_attachment_without_content_id_falls_back_to_the_filename() {
    let mut a = attachment("logo.png", vec![0x89, 0x50], Some("image/png"));
    a.inline = true;
    let mime = compose(&email(vec![a]));
    assert!(mime.contains("Content-ID: <logo.png>"), "{mime}");
}

/// The fallback must go through the **Content-ID** allowlist, not just
/// inherit `attachment_filename`'s output — that one keeps spaces, `<`,
/// `>`, `:` and every non-ASCII character. This is the door beside the
/// sanitizer, and it is reachable with no `contentId` supplied at all.
#[test]
fn a_non_ascii_filename_cannot_leak_into_content_id() {
    let mut a = attachment("Café Logo.png", vec![0x89, 0x50], Some("image/png"));
    a.inline = true;
    // ASCII is asserted by the helper — the point here is the sanitized id.
    let mime =
        String::from_utf8(compose_with(Some(FROM_NAME), FROM_ADDR, &email(vec![a]))).unwrap();
    assert!(mime.contains("Content-ID: <CafLogo.png>"), "{mime}");
}

#[test]
fn angle_brackets_in_a_filename_cannot_leak_into_content_id() {
    let mut a = attachment("logo<1>.png", vec![0x89, 0x50], Some("image/png"));
    a.inline = true;
    let mime = compose(&email(vec![a]));
    assert!(mime.contains("Content-ID: <logo1.png>"), "{mime}");
    assert!(!mime.contains("<logo<"), "{mime}");
}

/// RFC 2387 §3.1: "The type parameter must be specified and its value is
/// the MIME media type of the root body part."
#[test]
fn related_declares_the_root_body_type() {
    for (html, text, expected) in [
        (Some("<p>hi</p>"), Some("hi"), "multipart/alternative"),
        (Some("<p>hi</p>"), None, "text/html"),
        (None, Some("hi"), "text/plain"),
    ] {
        let mut a = attachment("logo.png", vec![0x89, 0x50], Some("image/png"));
        a.inline = true;
        let mut msg = email(vec![a]);
        msg.html = html.map(str::to_string);
        msg.text = text.map(str::to_string);
        let mime = compose(&msg);
        assert!(
            mime.contains(&format!(r#"type="{expected}""#)),
            "expected type=\"{expected}\" on the related container:\n{mime}"
        );
        // The boundary lettre generated must survive the header rewrite,
        // or the parts become unreachable.
        // The Content-Type is folded across continuation lines, so read
        // forward from the container rather than parsing a single line.
        let after = &mime[mime
            .find("multipart/related")
            .unwrap_or_else(|| panic!("no related container:\n{mime}"))..];
        let boundary = after
            .split("boundary=\"")
            .nth(1)
            .and_then(|b| b.split('"').next())
            .unwrap_or_else(|| panic!("related lost its boundary:\n{mime}"));
        assert!(
            mime.contains(&format!("--{boundary}--")),
            "the related container must still close its boundary:\n{mime}"
        );
    }
}

/// Two inline parts with the same filename would otherwise both claim
/// `<logo.png>` and `cid:logo.png` would resolve to whichever the client
/// indexed first — a plausible pair from a loop over per-row images.
#[test]
fn duplicate_inline_filenames_get_distinct_content_ids() {
    let inline = |name: &str| {
        let mut a = attachment(name, vec![0x89, 0x50], Some("image/png"));
        a.inline = true;
        a
    };
    let mime = compose(&email(vec![inline("logo.png"), inline("logo.png")]));
    assert!(mime.contains("Content-ID: <logo.png>"), "{mime}");
    assert!(mime.contains("Content-ID: <logo.png.1>"), "{mime}");
}

/// The positional escape hatch collides too: `["☕", "attachment0"]` both
/// reduce to `attachment0`.
#[test]
fn the_positional_fallback_cannot_collide_either() {
    let inline = |name: &str| {
        let mut a = attachment(name, vec![0x89, 0x50], Some("image/png"));
        a.inline = true;
        a
    };
    let mime = compose(&email(vec![inline("☕"), inline("attachment0")]));
    let ids = content_ids(&mime);
    assert_eq!(ids.len(), 2, "{mime}");
    assert_ne!(ids[0], ids[1], "content ids must be distinct: {ids:?}");
}

/// The suffix itself has to be checked, not just the base. With contentIds
/// ["logo", "logo.2", "logo"], the third part's base collides and the naive
/// `{base}.{idx}` lands on "logo.2" — a string the second part already took.
/// The filename path reaches the same place, since the allowlist keeps `.`
/// and digits.
#[test]
fn a_dedup_suffix_cannot_collide_with_an_existing_id() {
    let inline = |name: &str, cid: &str| {
        let mut a = attachment(name, vec![0x89, 0x50], Some("image/png"));
        a.inline = true;
        a.content_id = Some(cid.to_string());
        a
    };
    let mime = compose(&email(vec![
        inline("a.png", "logo"),
        inline("b.png", "logo.2"),
        inline("c.png", "logo"),
    ]));
    assert_eq!(
        content_ids(&mime).len(),
        3,
        "every inline part needs an id:\n{mime}"
    );
    let mut unique = content_ids(&mime);
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 3, "ids must be distinct: {unique:?}\n{mime}");
}

/// Same hazard reached through the filename-derived path, no contentId at all.
#[test]
fn filename_derived_ids_cannot_collide_with_a_suffix_either() {
    let inline = |name: &str| {
        let mut a = attachment(name, vec![0x89, 0x50], Some("image/png"));
        a.inline = true;
        a
    };
    let mime = compose(&email(vec![
        inline("logo.png"),
        inline("logo.png.1"),
        inline("logo.png"),
    ]));
    let mut ids = content_ids(&mime);
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "ids must be distinct: {ids:?}\n{mime}");
}

/// An author-supplied contentId shared by two parts is the same hazard.
#[test]
fn duplicate_author_supplied_content_ids_are_disambiguated() {
    let inline = |name: &str| {
        let mut a = attachment(name, vec![0x89, 0x50], Some("image/png"));
        a.inline = true;
        a.content_id = Some("logo".to_string());
        a
    };
    let mime = compose(&email(vec![inline("a.png"), inline("b.png")]));
    assert!(mime.contains("Content-ID: <logo>"), "{mime}");
    assert!(mime.contains("Content-ID: <logo.1>"), "{mime}");
}

/// A filename with nothing token-safe in it still needs a usable id.
#[test]
fn a_filename_with_no_token_characters_falls_back_to_a_positional_id() {
    let mut a = attachment("☕", vec![0x89, 0x50], Some("image/png"));
    a.inline = true;
    let mime =
        String::from_utf8(compose_with(Some(FROM_NAME), FROM_ADDR, &email(vec![a]))).unwrap();
    assert!(mime.contains("Content-ID: <attachment0>"), "{mime}");
}

/// RFC 2387: `cid:` resolution is only well-defined inside
/// `multipart/related`. An inline image sitting in `mixed` as a sibling of
/// the body is what makes Outlook and Apple Mail render it as a download
/// instead of in place.
#[test]
fn inline_attachments_ride_multipart_related() {
    let mut a = attachment("logo.png", vec![0x89, 0x50], Some("image/png"));
    a.inline = true;
    a.content_id = Some("logo".to_string());
    let mut msg = email(vec![a]);
    msg.html = Some(r#"<img src="cid:logo">"#.to_string());
    let mime = compose(&msg);

    assert!(
        mime.contains("multipart/related"),
        "inline parts need a related container:\n{mime}"
    );
    let (headers, _) = part(&mime, "logo.png");
    assert!(headers.contains("Content-ID: <logo>"), "{headers}");
    // With nothing downloadable there is no reason to add a mixed layer.
    assert!(
        !mime.contains("multipart/mixed"),
        "an inline-only message needs no mixed layer:\n{mime}"
    );
}

/// Both kinds at once: related wraps the body + inline parts, and mixed
/// wraps that plus the downloads.
#[test]
fn inline_and_downloadable_attachments_nest_correctly() {
    let mut logo = attachment("logo.png", vec![0x89, 0x50], Some("image/png"));
    logo.inline = true;
    logo.content_id = Some("logo".to_string());
    let mut msg = email(vec![
        logo,
        attachment(
            "report.pdf",
            b"%PDF-1.7\x00\xFF".to_vec(),
            Some("application/pdf"),
        ),
    ]);
    msg.html = Some(r#"<img src="cid:logo">"#.to_string());
    let mime = compose(&msg);

    let mixed = mime.find("multipart/mixed").expect("mixed present");
    let related = mime.find("multipart/related").expect("related present");
    assert!(mixed < related, "mixed must wrap related:\n{mime}");

    // The `type` parameter has to survive being a NESTED container, not just a
    // top-level one — that is where a rewritten header would be lost if lettre
    // ever re-derived a child multipart's Content-Type at format time rather
    // than storing it.
    assert!(
        mime.contains(r#"type="multipart/alternative""#),
        "nested related must still declare its root type:\n{mime}"
    );

    // The download stays out of `related` — it is not part of the document.
    assert!(
        part(&mime, "report.pdf")
            .0
            .contains("Content-Disposition: attachment"),
        "{mime}"
    );
    assert_eq!(part_bytes(&mime, "report.pdf"), b"%PDF-1.7\x00\xFF");
}

#[test]
fn html_and_text_become_multipart_alternative() {
    let mut msg = email(vec![]);
    msg.html = Some("<p>hi</p>".to_string());
    let mime = compose(&msg);
    assert!(mime.contains("multipart/alternative"), "{mime}");
    assert!(mime.contains("text/plain"), "{mime}");
    assert!(mime.contains("text/html"), "{mime}");
}

/// With attachments the body must nest *inside* multipart/mixed, or clients
/// show the text as just another download.
#[test]
fn body_nests_inside_mixed_when_attachments_are_present() {
    let mut msg = email(vec![attachment("a.txt", b"x".to_vec(), Some("text/plain"))]);
    msg.html = Some("<p>hi</p>".to_string());
    let mime = compose(&msg);
    let mixed = mime.find("multipart/mixed").expect("mixed present");
    let alternative = mime
        .find("multipart/alternative")
        .expect("alternative present");
    assert!(mixed < alternative, "mixed must wrap alternative:\n{mime}");
}

/// Blind copies must reach SES via the envelope, never the headers.
#[test]
fn bcc_never_appears_in_the_headers() {
    let mut msg = email(vec![]);
    msg.cc = vec!["cc@oxy.tech".to_string()];
    msg.bcc = vec!["secret@oxy.tech".to_string()];
    let mime = compose(&msg);
    assert!(mime.contains("Cc: cc@oxy.tech"), "{mime}");
    assert!(
        !mime.contains("secret@oxy.tech"),
        "a blind recipient must not be disclosed:\n{mime}"
    );
}

#[test]
fn multiple_attachments_all_survive() {
    let mime = compose(&email(vec![
        attachment("one.bin", vec![0x00, 0xFF], None),
        attachment("two.bin", vec![0xFE, 0x01], None),
    ]));
    for (name, bytes) in [
        ("one.bin", vec![0x00u8, 0xFF]),
        ("two.bin", vec![0xFE, 0x01]),
    ] {
        assert_eq!(part_bytes(&mime, name), bytes, "{name} must round-trip");
    }
}

// ---- content type ------------------------------------------------------

#[test]
fn unparseable_content_type_falls_back_to_octet_stream() {
    let mime = compose(&email(vec![attachment(
        "x.bin",
        vec![1, 2, 3],
        Some("not a mime type"),
    )]));
    assert!(mime.contains("application/octet-stream"), "{mime}");
}

#[test]
fn missing_content_type_falls_back_to_octet_stream() {
    let mime = compose(&email(vec![attachment("x.bin", vec![1, 2, 3], None)]));
    assert!(mime.contains("application/octet-stream"), "{mime}");
}

// ---- line length -----------------------------------------------------------

/// The hard `RawMessage` requirement `Content.Simple` used to satisfy for us.
///
/// Every value an author controls is made pathologically long at once: a
/// single-line HTML body far past the limit, an equally long plain-text
/// alternative, a 200-char non-ASCII filename (RFC 2231 continuations), a long
/// subject (RFC 2047 encoded-words) and a large attachment (base64 wrapping).
/// If lettre ever stops folding any one of them, this fails instead of SES.
#[test]
fn no_line_exceeds_the_rfc5322_limit() {
    let mut a = attachment(
        &"é".repeat(200),
        (0..20_000).map(|i| (i % 256) as u8).collect(),
        Some("application/octet-stream"),
    );
    a.inline = true;
    a.content_id = Some("x".repeat(300));

    let mut msg = email(vec![
        a,
        attachment(
            "report.csv",
            "x,y\n".repeat(5000).into_bytes(),
            Some("text/csv"),
        ),
    ]);
    msg.subject = "Rapport café — ".repeat(60);
    msg.text = Some("no newlines at all ".repeat(500));
    msg.html = Some(format!("<p>{}</p>", "long ".repeat(4000)));
    msg.cc = (0..20).map(|i| format!("person{i}@oxy.tech")).collect();

    // compose() asserts the limit; this pins that the fixture really is
    // hostile, so the test cannot pass by building something small.
    let mime = compose(&msg);
    assert!(mime.len() > 30_000, "fixture must be large: {}", mime.len());
}

/// A long unbroken run with no whitespace to fold at — the case a
/// naive folder gets wrong, since RFC 5322 folding needs a fold point.
#[test]
fn an_unfoldable_body_line_still_respects_the_limit() {
    let mut msg = email(vec![]);
    msg.text = None;
    msg.html = Some(format!("<p>{}</p>", "a".repeat(5000)));
    compose(&msg);
}
