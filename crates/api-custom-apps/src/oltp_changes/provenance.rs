//! Who made each change, signed (OLTP change stream design §5.2, "Who made
//! it, signed").
//!
//! Every capture trigger copies three transaction-local settings into each log
//! row it writes, beside the transaction id it reads itself:
//! `oxy_changes.actor`, `oxy_changes.invocation` and `oxy_changes.provenance`.
//! The host sets them at the start of every production `ctx.oltp` transaction
//! of an app whose build declares a table under `oltpChanges`
//! (`custom_apps_functions::host::oltp_sign`), and of each migration file such
//! an app applies to production (`custom_apps_migrations::apply`): it reads
//! the transaction id ([`TXID_SQL`]), then sets the three ([`SIGN_SQL`]), the
//! way `PUBLISH_WITH_INVOKER_SQL` sets a publish's invoker
//! (`oxy_oltp::events`).
//!
//! **The MAC.** `provenance` is an `HMAC-SHA256` over the transaction id, the
//! environment, the writer's schema, the actor and the invocation
//! ([`ChangeSigner`]), under a key of the org's own derived from the
//! deployment's master key exactly as the outbox's is
//! (`app_events::provenance::org_key_for`), under a domain of its own
//! ([`KEY_DOMAIN`]), so neither key can stand in for the other. The key never
//! enters the isolate or the store: the MAC is a bound parameter of the host's
//! statement, as hex the trigger decodes. The relay recomputes it from the
//! log row ([`verify`]); a change whose MAC is missing or wrong still ships,
//! as `unverified` and with no invocation.
//!
//! **Why the transaction id.** App SQL the oltp guard cannot see (built while
//! it runs) can set any custom setting, at any point in its transaction. A
//! setting it writes fails the MAC; a MAC copied from another transaction
//! names that transaction's id; and one read back from its own transaction is
//! already its own.
//!
//! **Who.** A function's run signs as the host's actor (the invoking person,
//! or the app's owner for a run no person started) with its invocation id; a
//! migration file as `migration:<file>` ([`migration_actor`]) with none.
//!
//! **The message.** A versioned domain, then every field length-prefixed —
//! its length as 8 bytes big-endian, then its bytes — as
//! `app_events::provenance` encodes one: the transaction id as its 8
//! big-endian bytes, the three texts as UTF-8, the invocation as its 16 bytes
//! or empty. No field's bytes can be read as another's.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

/// What the per-org key is derived under, before the org's id.
pub const KEY_DOMAIN: &[u8] = b"oxy-changes-provenance";
/// What every signed message starts with: a new encoding gets a new version.
const MESSAGE_DOMAIN: &[u8] = b"oxy-changes-provenance:v1\0";

/// The open transaction's id, assigning one when it has none yet: the id the
/// capture trigger stamps on each log row (`pg_current_xact_id()`). As text,
/// since `xid8` has no binary form a client reads; [`parse_txid`] reads it.
pub const TXID_SQL: &str = "SELECT pg_catalog.pg_current_xact_id()::text";

/// Who made the open transaction's changes: the actor (`$1`), the invocation
/// (`$2`, `''` for none) and the MAC as hex (`$3`), each set for the
/// transaction alone (`is_local`), so the session carries none of them into
/// its next one. Schema-qualified, so a `search_path` the app chose resolves
/// nothing here.
pub const SIGN_SQL: &str = "SELECT \
     pg_catalog.set_config('oxy_changes.actor', $1, true), \
     pg_catalog.set_config('oxy_changes.invocation', $2, true), \
     pg_catalog.set_config('oxy_changes.provenance', $3, true)";

/// What the host signs for one transaction, and what the relay reads back from
/// a log row to verify it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSigner<'a> {
    /// The transaction's id (`xid8`): what binds a MAC to one transaction.
    pub txid: u64,
    /// The environment the store serves: `production`.
    pub environment: &'a str,
    /// The writer's schema, `app_<writer>`, whose tables the transaction
    /// changes.
    pub writer: &'a str,
    /// Who made the changes: the run's actor id, or `migration:<file>`.
    pub actor: &'a str,
    /// The function invocation that made them; `None` for a migration.
    pub invocation: Option<Uuid>,
}

/// The host's MAC over `s`, under `org_id`'s key.
pub fn sign(org_id: Uuid, s: &ChangeSigner<'_>) -> [u8; 32] {
    sign_keyed(&org_key(org_id), s)
}

/// [`sign`] as the hex text [`SIGN_SQL`] sets (`$3`): the connector binds
/// text, never `bytea`, and the capture trigger decodes it.
pub fn sign_hex(org_id: Uuid, s: &ChangeSigner<'_>) -> String {
    hex::encode(sign(org_id, s))
}

/// Whether `mac` is the host's MAC over `s` under `org_id`'s key, compared in
/// constant time. A missing MAC never verifies.
pub fn verify(org_id: Uuid, s: &ChangeSigner<'_>, mac: Option<&[u8]>) -> bool {
    verify_keyed(&org_key(org_id), s, mac)
}

/// The actor a migration file's transaction is signed as.
pub fn migration_actor(filename: &str) -> String {
    format!("migration:{filename}")
}

/// The transaction id [`TXID_SQL`] returns, from its text; `None` for text
/// that is not one.
pub fn parse_txid(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

/// `org_id`'s key: `HMAC-SHA256(master, "oxy-changes-provenance" ‖ org id)`.
fn org_key(org_id: Uuid) -> [u8; 32] {
    crate::app_events::provenance::org_key_for(KEY_DOMAIN, org_id)
}

/// The MAC over `s` under an org's `key` ([`org_key`]).
pub(crate) fn sign_keyed(key: &[u8; 32], s: &ChangeSigner<'_>) -> [u8; 32] {
    let mut mac = keyed(key);
    mac.update(&message(s));
    let mut out = [0u8; 32];
    out.copy_from_slice(&mac.finalize().into_bytes());
    out
}

/// Whether `mac` is the MAC over `s` under an org's `key`, in constant time.
pub(crate) fn verify_keyed(key: &[u8; 32], s: &ChangeSigner<'_>, mac: Option<&[u8]>) -> bool {
    let Some(given) = mac else {
        return false;
    };
    let mut mac = keyed(key);
    mac.update(&message(s));
    // Constant-time, and a slice of any other length never verifies.
    mac.verify_slice(given).is_ok()
}

fn keyed(key: &[u8]) -> HmacSha256 {
    // HMAC takes a key of any length; a 32-byte one is never refused.
    HmacSha256::new_from_slice(key).expect("HMAC accepts any key length")
}

/// The canonical encoding of `s`: the versioned domain, then every field in a
/// fixed order, each length-prefixed.
fn message(s: &ChangeSigner<'_>) -> Vec<u8> {
    let mut out = Vec::with_capacity(160);
    out.extend_from_slice(MESSAGE_DOMAIN);
    field(&mut out, &s.txid.to_be_bytes());
    field(&mut out, s.environment.as_bytes());
    field(&mut out, s.writer.as_bytes());
    field(&mut out, s.actor.as_bytes());
    match &s.invocation {
        Some(id) => field(&mut out, id.as_bytes()),
        None => field(&mut out, &[]),
    }
    out
}

fn field(out: &mut Vec<u8>, bytes: &[u8]) {
    out.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_events::provenance as app_events;

    const MASTER: [u8; 32] = [7; 32];

    fn org() -> Uuid {
        Uuid::from_u128(0x11)
    }

    /// `org_id`'s key under [`MASTER`], as [`org_key`] derives it under the
    /// deployment's master key.
    fn key(org_id: Uuid) -> [u8; 32] {
        app_events::org_key_with(&MASTER, KEY_DOMAIN, org_id)
    }

    fn signer() -> ChangeSigner<'static> {
        ChangeSigner {
            txid: 742,
            environment: "production",
            writer: "app_orders",
            actor: "6f1c0d2e-0000-4000-8000-000000000001",
            invocation: Some(Uuid::from_u128(5)),
        }
    }

    /// A MAC copied into another transaction names the wrong id.
    #[test]
    fn a_mac_verifies_only_its_own_txid() {
        let mac = sign_keyed(&key(org()), &signer());
        assert!(verify_keyed(&key(org()), &signer(), Some(&mac)));
        for txid in [741, 743, 0, u64::MAX] {
            let other = ChangeSigner { txid, ..signer() };
            assert!(
                !verify_keyed(&key(org()), &other, Some(&mac)),
                "txid {txid} verified"
            );
        }
    }

    #[test]
    fn a_mac_does_not_verify_under_another_org() {
        let mac = sign_keyed(&key(org()), &signer());
        let other = Uuid::from_u128(0x12);
        assert_ne!(key(org()), key(other));
        assert!(!verify_keyed(&key(other), &signer(), Some(&mac)));
        // Nor under another deployment's master key.
        let foreign = app_events::org_key_with(&[8; 32], KEY_DOMAIN, org());
        assert!(!verify_keyed(&foreign, &signer(), Some(&mac)));
    }

    #[test]
    fn each_field_changes_the_mac() {
        let s = signer;
        let mac = sign_keyed(&key(org()), &s());
        for (what, changed) in [
            ("txid", ChangeSigner { txid: 743, ..s() }),
            (
                "environment",
                ChangeSigner {
                    environment: "staging",
                    ..s()
                },
            ),
            (
                "writer",
                ChangeSigner {
                    writer: "app_other",
                    ..s()
                },
            ),
            (
                "actor",
                ChangeSigner {
                    actor: "migration:0001_init.sql",
                    ..s()
                },
            ),
            (
                "invocation",
                ChangeSigner {
                    invocation: Some(Uuid::from_u128(9)),
                    ..s()
                },
            ),
            (
                "no invocation",
                ChangeSigner {
                    invocation: None,
                    ..s()
                },
            ),
        ] {
            assert!(
                !verify_keyed(&key(org()), &changed, Some(&mac)),
                "a changed {what} still verifies"
            );
        }
    }

    /// The same master key, org and fields: a key of its own, never the
    /// outbox's, so a MAC under one never verifies under the other.
    #[test]
    fn the_domain_differs_from_app_events() {
        assert_ne!(KEY_DOMAIN, app_events::KEY_DOMAIN);
        let outbox = app_events::org_key_with(&MASTER, app_events::KEY_DOMAIN, org());
        assert_ne!(key(org()), outbox);
        let under_the_outbox_key = sign_keyed(&outbox, &signer());
        assert!(!verify_keyed(
            &key(org()),
            &signer(),
            Some(&under_the_outbox_key)
        ));
    }

    /// Length-prefixed: moving bytes from one field to the next is another
    /// message, and a migration's absent invocation is not any invocation.
    #[test]
    fn the_encoding_is_unambiguous() {
        let a = ChangeSigner {
            writer: "app_ab",
            actor: "c",
            ..signer()
        };
        let b = ChangeSigner {
            writer: "app_a",
            actor: "bc",
            ..signer()
        };
        assert_ne!(message(&a), message(&b));
        let none = ChangeSigner {
            invocation: None,
            ..signer()
        };
        let nil = ChangeSigner {
            invocation: Some(Uuid::nil()),
            ..signer()
        };
        assert_ne!(message(&none), message(&nil));
        assert!(message(&signer()).starts_with(b"oxy-changes-provenance:v1\0"));
    }

    #[test]
    fn a_missing_short_or_hex_mac_fails() {
        let mac = sign_keyed(&key(org()), &signer());
        assert!(!verify_keyed(&key(org()), &signer(), None));
        assert!(!verify_keyed(&key(org()), &signer(), Some(&[])));
        assert!(!verify_keyed(&key(org()), &signer(), Some(&mac[..16])));
        // The setting carries hex, which the trigger decodes: the text itself
        // is not the MAC.
        let as_hex = hex::encode(mac);
        assert!(!verify_keyed(
            &key(org()),
            &signer(),
            Some(as_hex.as_bytes())
        ));
    }

    /// The key stays out of the SQL: the three settings platform step 8 names
    /// come from bound parameters, for the transaction alone.
    #[test]
    fn the_settings_are_set_for_the_transaction_from_bound_parameters() {
        use oxy_oltp::platform::{
            CHANGES_ACTOR_SETTING, CHANGES_INVOCATION_SETTING, CHANGES_PROVENANCE_SETTING,
        };
        for (setting, param) in [
            (CHANGES_ACTOR_SETTING, "$1"),
            (CHANGES_INVOCATION_SETTING, "$2"),
            (CHANGES_PROVENANCE_SETTING, "$3"),
        ] {
            assert!(
                SIGN_SQL.contains(&format!(
                    "pg_catalog.set_config('{setting}', {param}, true)"
                )),
                "{setting}: {SIGN_SQL}"
            );
        }
        assert_eq!(SIGN_SQL.matches('$').count(), 3);
        assert_eq!(TXID_SQL, "SELECT pg_catalog.pg_current_xact_id()::text");
    }

    #[test]
    fn txids_and_migration_actors_read_back() {
        assert_eq!(parse_txid("742"), Some(742));
        assert_eq!(parse_txid(" 18446744073709551615\n"), Some(u64::MAX));
        assert_eq!(parse_txid(""), None);
        assert_eq!(parse_txid("-1"), None);
        assert_eq!(
            migration_actor("0002_orders.sql"),
            "migration:0002_orders.sql"
        );
    }
}
