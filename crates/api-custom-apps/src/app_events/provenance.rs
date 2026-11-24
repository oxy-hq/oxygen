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
//!
//! **Who a send ran as: a second MAC, message domain v2.** A `ctx.apps.send`
//! made by a run that ran directly as a person carries that fact and the
//! credential the person ran under ([`Invoker`]: a login session, or a stored
//! token by its id) in three more columns (platform step 7). The host signs
//! them with [`sign_invoker`]: the v1 encoding of every field above under a
//! domain of its own (`…:v2`), then the invoker. It is a second MAC beside the
//! first, not a replacement, so a relay one release back still verifies the
//! row and delivers it as the system, as every send was delivered before; and
//! a row whose invoker MAC is missing or not the host's is still the host's
//! row, delivered as the system ([`verified_invoker`]). Only a verified
//! invoker MAC makes a delivery run as a person, and a v1-only row never does.

use hmac::{Hmac, KeyInit, Mac};
use oxy_oltp::events::OutboxRow;
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

/// The stable code a quarantined row's `relay_error` starts with.
pub const UNVERIFIED_PROVENANCE: &str = "unverified_provenance";

/// What the per-org key is derived under, before the org's id. Visible to the
/// crate so another stream's key (`oltp_changes::provenance`) can be shown to
/// differ from this one.
pub(crate) const KEY_DOMAIN: &[u8] = b"app-events-provenance";
/// What every signed message starts with: a new encoding gets a new version.
const MESSAGE_DOMAIN: &[u8] = b"oxy-app-events-provenance:v1\0";
/// What the invoker MAC's message starts with: the v1 fields, then the
/// invoker ([`sign_invoker`]). A domain of its own, so neither MAC can ever
/// pass for the other.
const MESSAGE_DOMAIN_V2: &[u8] = b"oxy-app-events-provenance:v2\0";

/// Who a `ctx.apps.send`'s run ran as, when it ran directly as a person: the
/// credential the person ran under, by reference, which a delivery re-reads
/// to stand that person up again with the same ceiling
/// (`app_function_executor::sender`). A run that did not run as a person
/// (an event's delivery, a schedule, a send delivered as the system) has
/// none, and its sends run as the system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invoker {
    /// A login session: no token narrowed the person.
    Session,
    /// A stored token (`api_tokens.id`): a personal, agent, sandbox agent or
    /// service account token, or the token behind a token session.
    Token(Uuid),
}

impl Invoker {
    /// `session` or `token`: the `invoker` column.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Session => "session",
            Self::Token(_) => "token",
        }
    }

    /// The `invoker_token_id` column.
    pub fn token_id(&self) -> Option<Uuid> {
        match self {
            Self::Session => None,
            Self::Token(id) => Some(*id),
        }
    }

    /// The invoker two stored columns name, or `None` when they name none
    /// or are not one well-formed invoker (a kind that is neither, a token
    /// kind without an id, a session with one).
    pub fn from_columns(kind: Option<&str>, token_id: Option<Uuid>) -> Option<Self> {
        match (kind, token_id) {
            (Some("session"), None) => Some(Self::Session),
            (Some("token"), Some(id)) => Some(Self::Token(id)),
            _ => None,
        }
    }
}

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

/// The host's invoker MAC over `fields` and `invoker`, under `org_id`'s key:
/// message domain v2 (see the module docs).
pub fn sign_invoker(org_id: Uuid, fields: &Provenance<'_>, invoker: Invoker) -> [u8; 32] {
    mac_over(&master_key(), org_id, &message_v2(fields, invoker))
}

/// [`sign_invoker`] as the hex text the publish statement passes the definer
/// (`PUBLISH_WITH_INVOKER_SQL`'s last parameter).
pub fn sign_invoker_hex(org_id: Uuid, fields: &Provenance<'_>, invoker: Invoker) -> String {
    hex::encode(sign_invoker(org_id, fields, invoker))
}

/// Whether `mac` is the host's invoker MAC over `fields` and `invoker`,
/// compared in constant time. A missing MAC never verifies.
pub fn verify_invoker(
    org_id: Uuid,
    fields: &Provenance<'_>,
    invoker: Invoker,
    mac: Option<&[u8]>,
) -> bool {
    verify_over(&master_key(), org_id, &message_v2(fields, invoker), mac)
}

/// The invoker `row` carries, when the host signed it: its columns name one
/// well-formed [`Invoker`] and its invoker MAC is the host's over `fields`
/// (the row's own, read by [`Provenance::of_row`]) and that invoker. `None`
/// for a row that names none, and for one whose invoker the host did not
/// sign: a row a server one release back wrote (v1 only), or one whose
/// columns app SQL filled through the definer's settings. Such a row is
/// still the host's when its v1 MAC verifies, and is delivered as the
/// system.
pub fn verified_invoker(org_id: Uuid, fields: &Provenance<'_>, row: &OutboxRow) -> Option<Invoker> {
    verified_invoker_with(&master_key(), org_id, fields, row)
}

fn verified_invoker_with(
    master: &[u8; 32],
    org_id: Uuid,
    fields: &Provenance<'_>,
    row: &OutboxRow,
) -> Option<Invoker> {
    let invoker = Invoker::from_columns(row.invoker.as_deref(), row.invoker_token_id)?;
    let message = message_v2(fields, invoker);
    verify_over(master, org_id, &message, row.invoker_provenance.as_deref()).then_some(invoker)
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
    org_key_with(master, KEY_DOMAIN, org_id)
}

/// `org_id`'s key under `domain`, from the deployment's master key:
/// `HMAC-SHA256(master, domain ‖ org id)`, as [`org_key`] derives the
/// outbox's under [`KEY_DOMAIN`]. For another stream that signs who wrote it
/// (`oltp_changes::provenance`), under a domain of its own, so neither key
/// can stand in for the other. The key never leaves the host process.
pub(crate) fn org_key_for(domain: &[u8], org_id: Uuid) -> [u8; 32] {
    org_key_with(&master_key(), domain, org_id)
}

/// [`org_key_for`] under `master`: what a test derives without reading the
/// deployment's key.
pub(crate) fn org_key_with(master: &[u8; 32], domain: &[u8], org_id: Uuid) -> [u8; 32] {
    let mut mac = keyed(master);
    mac.update(domain);
    mac.update(org_id.as_bytes());
    to_array(&mac.finalize().into_bytes())
}

fn sign_with(master: &[u8; 32], org_id: Uuid, fields: &Provenance<'_>) -> [u8; 32] {
    mac_over(master, org_id, &message(fields))
}

fn verify_with(
    master: &[u8; 32],
    org_id: Uuid,
    fields: &Provenance<'_>,
    mac: Option<&[u8]>,
) -> bool {
    verify_over(master, org_id, &message(fields), mac)
}

/// The MAC over `message` under `org_id`'s key.
fn mac_over(master: &[u8; 32], org_id: Uuid, message: &[u8]) -> [u8; 32] {
    let mut mac = keyed(&org_key(master, org_id));
    mac.update(message);
    to_array(&mac.finalize().into_bytes())
}

/// Whether `mac` is the MAC over `message` under `org_id`'s key.
fn verify_over(master: &[u8; 32], org_id: Uuid, message: &[u8], mac: Option<&[u8]>) -> bool {
    let Some(given) = mac else {
        return false;
    };
    let mut mac = keyed(&org_key(master, org_id));
    mac.update(message);
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
    encode_fields(&mut out, fields);
    out
}

/// The invoker MAC's message: domain v2, the v1 encoding of `fields`, then the
/// invoker's kind as a text and its token id as an optional.
fn message_v2(fields: &Provenance<'_>, invoker: Invoker) -> Vec<u8> {
    let mut out = Vec::with_capacity(320);
    out.extend_from_slice(MESSAGE_DOMAIN_V2);
    encode_fields(&mut out, fields);
    field(&mut out, invoker.kind().as_bytes());
    let token = invoker.token_id();
    optional(&mut out, token.as_ref().map(|id| &id.as_bytes()[..]));
    out
}

/// Every field of `fields`, in [`message`]'s order and encoding.
fn encode_fields(out: &mut Vec<u8>, fields: &Provenance<'_>) {
    field(out, fields.id.as_bytes());
    field(out, fields.environment.as_bytes());
    field(out, fields.event_type.as_bytes());
    field(out, fields.key.as_bytes());
    field(out, fields.writer_role.as_bytes());
    optional(out, fields.actor.map(str::as_bytes));
    field(out, fields.root_id.as_bytes());
    optional(
        out,
        fields.causation_id.as_ref().map(|id| &id.as_bytes()[..]),
    );
    out.extend_from_slice(&fields.depth.to_be_bytes());
    out.extend_from_slice(&fields.delay_seconds.to_be_bytes());
    out.extend_from_slice(&fields.data_bytes.to_be_bytes());
    optional(
        out,
        fields.schema_revision.as_ref().map(|id| &id.as_bytes()[..]),
    );
    optional(
        out,
        fields.invocation_id.as_ref().map(|id| &id.as_bytes()[..]),
    );
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
