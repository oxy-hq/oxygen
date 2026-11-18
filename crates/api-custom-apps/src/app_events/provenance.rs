//! Provenance: the relay delivers only what the host wrote (app events design
//! §6.2, ruling R25(c)).
//!
//! An app's writer role holds `EXECUTE` on `oxy_events.publish`, so its own
//! SQL — a `DO` block, or a function the app defined — can write an outbox row
//! the host never admitted: a type the calling function was not granted, a
//! forged `actor`, `root_id` or `causation_id`, or a row written while the
//! org's `app_events` flag was off. The definer bounds the row and the relay
//! re-derives what it can, but those it cannot.
//!
//! So the host signs every row it writes, after every admission check passed:
//! an `HMAC-SHA256`, under a key of the org's own ([`org_key`]), over a
//! canonical, length-prefixed encoding of every field of the row the relay
//! trusts ([`Provenance`]). The MAC goes to the definer with the row and is
//! stored beside it (platform step 6); the relay recomputes it from the row's
//! fields and compares in constant time ([`verify`]). A row whose MAC is
//! missing or wrong is stored with a `relay_error` starting
//! [`UNVERIFIED_PROVENANCE`], delivered to nobody, and its body kept.
//!
//! **The key.** Derived per org from the deployment's master key — the key
//! `oxy_platform::secrets` seals app and org secrets with at rest
//! (`OXY_ENCRYPTION_KEY`, or the KMS-sealed `server_keys` row) — as
//! `HMAC-SHA256(master, "app-events-provenance" ‖ org id)`. Every serve and
//! worker process of a deployment already holds that one key, and a boot check
//! refuses a process whose key differs, so a host on one pod and a relay on
//! another agree. It never enters the isolate, the org's store or a log: the
//! MAC is a bound parameter of the host's own statement.
//!
//! **What is signed.** The event id, environment, qualified type, key, writer
//! role, actor, root, causation, depth, delay, the body's size as `jsonb` text
//! (the size the store reports, `octet_length(data::text)`: the host measures
//! a payload exactly so before sending it), the schema revision and the
//! invocation. Not the body: the relay never reads one, and no row can be
//! changed after the definer writes it (`ON CONFLICT DO NOTHING`, and a writer
//! holds nothing on the table), so the size binds a host's row to the body it
//! sent.

use hmac::{Hmac, KeyInit, Mac};
use oxy_oltp::events::OutboxRow;
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

/// The stable code a quarantined row's `relay_error` starts with.
pub const UNVERIFIED_PROVENANCE: &str = "unverified_provenance";

/// What the per-org key is derived under, before the org's id.
const KEY_DOMAIN: &[u8] = b"app-events-provenance";
/// What every signed message starts with: a new encoding gets a new version.
const MESSAGE_DOMAIN: &[u8] = b"oxy-app-events-provenance:v1\0";

/// Every field of an outbox row the relay trusts, as the host signs it and
/// the relay reads it back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance<'a> {
    pub id: Uuid,
    pub environment: &'a str,
    /// `<app slug>/<name>`: the relay derives it from the writer's app.
    pub event_type: &'a str,
    pub key: &'a str,
    /// The role the definer records (`session_user`).
    pub writer_role: &'a str,
    pub actor: Option<&'a str>,
    pub root_id: Uuid,
    pub causation_id: Option<Uuid>,
    pub depth: i32,
    pub delay_seconds: i32,
    /// The body's size as `jsonb` text.
    pub data_bytes: i64,
    pub schema_revision: Option<Uuid>,
    pub invocation_id: Option<Uuid>,
}

impl<'a> Provenance<'a> {
    /// `row`'s fields, its type qualified by its writer's app (`event_type`).
    pub fn of_row(row: &'a OutboxRow, event_type: &'a str) -> Self {
        Self {
            id: row.id,
            environment: &row.environment,
            event_type,
            key: &row.key,
            writer_role: &row.writer_role,
            actor: row.actor.as_deref(),
            root_id: row.root_id,
            causation_id: row.causation_id,
            depth: row.depth,
            delay_seconds: row.delay_seconds,
            data_bytes: i64::from(row.data_bytes),
            schema_revision: row.schema_revision,
            invocation_id: row.invocation_id,
        }
    }
}

/// The host's MAC over `fields`, under `org_id`'s key.
pub fn sign(org_id: Uuid, fields: &Provenance<'_>) -> [u8; 32] {
    sign_with(&master_key(), org_id, fields)
}

/// [`sign`] as the hex text the definer's call decodes (`PUBLISH_SQL`'s last
/// parameter: the connector binds text, never `bytea`).
pub fn sign_hex(org_id: Uuid, fields: &Provenance<'_>) -> String {
    hex::encode(sign(org_id, fields))
}

/// Whether `mac` is the host's MAC over `fields` under `org_id`'s key,
/// compared in constant time. A missing MAC never verifies.
pub fn verify(org_id: Uuid, fields: &Provenance<'_>, mac: Option<&[u8]>) -> bool {
    verify_with(&master_key(), org_id, fields, mac)
}

/// Whether a stored event's `relay_error` is a provenance refusal.
pub fn is_unverified(relay_error: &str) -> bool {
    relay_error.starts_with(UNVERIFIED_PROVENANCE)
}

/// The deployment's master key, which seals secrets at rest.
fn master_key() -> [u8; 32] {
    oxy::utils::get_encryption_key()
}

/// `org_id`'s key: `HMAC-SHA256(master, "app-events-provenance" ‖ org id)`.
/// The org's id is a fixed 16 bytes, so the input is unambiguous.
fn org_key(master: &[u8; 32], org_id: Uuid) -> [u8; 32] {
    let mut mac = keyed(master);
    mac.update(KEY_DOMAIN);
    mac.update(org_id.as_bytes());
    to_array(&mac.finalize().into_bytes())
}

fn sign_with(master: &[u8; 32], org_id: Uuid, fields: &Provenance<'_>) -> [u8; 32] {
    let mut mac = keyed(&org_key(master, org_id));
    mac.update(&message(fields));
    to_array(&mac.finalize().into_bytes())
}

fn verify_with(
    master: &[u8; 32],
    org_id: Uuid,
    fields: &Provenance<'_>,
    mac: Option<&[u8]>,
) -> bool {
    let Some(given) = mac else {
        return false;
    };
    let mut mac = keyed(&org_key(master, org_id));
    mac.update(&message(fields));
    // Constant-time, and a slice of any other length never verifies.
    mac.verify_slice(given).is_ok()
}

fn keyed(key: &[u8]) -> HmacSha256 {
    // HMAC takes a key of any length; a 32-byte one is never refused.
    HmacSha256::new_from_slice(key).expect("HMAC accepts any key length")
}

fn to_array(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(bytes);
    out
}

/// The canonical encoding of `fields`: a versioned domain, then every field
/// in a fixed order — a text or an id as its length (8 bytes, big-endian)
/// and its bytes, an optional as `0`, or `1` and the field, a number as its
/// fixed-width big-endian bytes. No field's bytes can be read as another's.
fn message(fields: &Provenance<'_>) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(MESSAGE_DOMAIN);
    field(&mut out, fields.id.as_bytes());
    field(&mut out, fields.environment.as_bytes());
    field(&mut out, fields.event_type.as_bytes());
    field(&mut out, fields.key.as_bytes());
    field(&mut out, fields.writer_role.as_bytes());
    optional(&mut out, fields.actor.map(str::as_bytes));
    field(&mut out, fields.root_id.as_bytes());
    optional(
        &mut out,
        fields.causation_id.as_ref().map(|id| &id.as_bytes()[..]),
    );
    out.extend_from_slice(&fields.depth.to_be_bytes());
    out.extend_from_slice(&fields.delay_seconds.to_be_bytes());
    out.extend_from_slice(&fields.data_bytes.to_be_bytes());
    optional(
        &mut out,
        fields.schema_revision.as_ref().map(|id| &id.as_bytes()[..]),
    );
    optional(
        &mut out,
        fields.invocation_id.as_ref().map(|id| &id.as_bytes()[..]),
    );
    out
}

fn field(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

fn optional(out: &mut Vec<u8>, bytes: Option<&[u8]>) {
    match bytes {
        None => out.push(0),
        Some(bytes) => {
            out.push(1);
            field(out, bytes);
        }
    }
}

#[cfg(test)]
mod tests;
