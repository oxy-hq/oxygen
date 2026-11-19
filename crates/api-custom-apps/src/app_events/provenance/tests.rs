use super::*;

const MASTER: [u8; 32] = [7; 32];

fn org() -> Uuid {
    Uuid::from_u128(0x11)
}

fn fields() -> Provenance<'static> {
    Provenance {
        id: Uuid::from_u128(1),
        environment: "production",
        event_type: "locations/statement.uploaded",
        key: "statement:42",
        writer_role: "app_locations_rw",
        actor: Some("user-1"),
        root_id: Uuid::from_u128(2),
        causation_id: Some(Uuid::from_u128(3)),
        depth: 1,
        delay_seconds: 30,
        data_bytes: 64,
        schema_revision: Some(Uuid::from_u128(4)),
        invocation_id: Some(Uuid::from_u128(5)),
    }
}

/// Every change to one signed field: each must fail the MAC signed over
/// [`fields`].
fn each_field_changed() -> Vec<(&'static str, Provenance<'static>)> {
    let f = fields;
    vec![
        (
            "id",
            Provenance {
                id: Uuid::from_u128(9),
                ..f()
            },
        ),
        (
            "environment",
            Provenance {
                environment: "staging",
                ..f()
            },
        ),
        (
            "type",
            Provenance {
                event_type: "locations/other",
                ..f()
            },
        ),
        (
            "key",
            Provenance {
                key: "statement:43",
                ..f()
            },
        ),
        (
            "writer",
            Provenance {
                writer_role: "app_other_rw",
                ..f()
            },
        ),
        (
            "actor",
            Provenance {
                actor: Some("user-2"),
                ..f()
            },
        ),
        ("no actor", Provenance { actor: None, ..f() }),
        (
            "root",
            Provenance {
                root_id: Uuid::from_u128(9),
                ..f()
            },
        ),
        (
            "cause",
            Provenance {
                causation_id: Some(Uuid::from_u128(9)),
                ..f()
            },
        ),
        (
            "no cause",
            Provenance {
                causation_id: None,
                ..f()
            },
        ),
        ("depth", Provenance { depth: 2, ..f() }),
        (
            "delay",
            Provenance {
                delay_seconds: 31,
                ..f()
            },
        ),
        (
            "size",
            Provenance {
                data_bytes: 65,
                ..f()
            },
        ),
        (
            "revision",
            Provenance {
                schema_revision: None,
                ..f()
            },
        ),
        (
            "invocation",
            Provenance {
                invocation_id: Some(Uuid::from_u128(9)),
                ..f()
            },
        ),
    ]
}

#[test]
fn the_hosts_mac_verifies() {
    let mac = sign_with(&MASTER, org(), &fields());
    assert!(verify_with(&MASTER, org(), &fields(), Some(&mac)));
}

/// R25(c): a row whose actor, root, causation or any other signed field
/// differs from what the host signed does not verify.
#[test]
fn a_change_to_any_signed_field_fails() {
    let mac = sign_with(&MASTER, org(), &fields());
    for (what, changed) in each_field_changed() {
        assert!(
            !verify_with(&MASTER, org(), &changed, Some(&mac)),
            "a changed {what} still verifies"
        );
    }
}

#[test]
fn a_missing_short_or_foreign_mac_fails() {
    let mac = sign_with(&MASTER, org(), &fields());
    assert!(!verify_with(&MASTER, org(), &fields(), None));
    assert!(!verify_with(&MASTER, org(), &fields(), Some(&[])));
    assert!(!verify_with(&MASTER, org(), &fields(), Some(&mac[..16])));
    assert!(!verify_with(&MASTER, org(), &fields(), Some(&[0; 32])));
    // Another org's key, or another deployment's.
    assert!(!verify_with(
        &MASTER,
        Uuid::from_u128(0x12),
        &fields(),
        Some(&mac)
    ));
    assert!(!verify_with(&[8; 32], org(), &fields(), Some(&mac)));
}

/// Length-prefixed: moving bytes from one field to the next is a different
/// message, and an absent optional is not an empty one.
#[test]
fn the_encoding_is_unambiguous() {
    let a = Provenance {
        environment: "dev-a",
        key: "bc",
        ..fields()
    };
    let b = Provenance {
        environment: "dev-ab",
        key: "c",
        ..fields()
    };
    assert_ne!(sign_with(&MASTER, org(), &a), sign_with(&MASTER, org(), &b));
    let none = Provenance {
        actor: None,
        ..fields()
    };
    let empty = Provenance {
        actor: Some(""),
        ..fields()
    };
    assert_ne!(
        sign_with(&MASTER, org(), &none),
        sign_with(&MASTER, org(), &empty)
    );
}

/// The key is the org's own: one org's MAC never verifies for another, and
/// the master key is never used as the MAC key itself.
#[test]
fn each_org_has_its_own_key() {
    assert_ne!(
        org_key(&MASTER, org()),
        org_key(&MASTER, Uuid::from_u128(0x12))
    );
    assert_ne!(org_key(&MASTER, org()), MASTER);
}

#[test]
fn only_the_stable_code_marks_a_provenance_refusal() {
    assert!(is_unverified(&format!("{UNVERIFIED_PROVENANCE}: no MAC")));
    assert!(!is_unverified("published under id 1, which is not the id"));
}

// ── The invoker MAC (message domain v2) ─────────────────────────────────────

fn token() -> Invoker {
    Invoker::Token(Uuid::from_u128(0x70))
}

fn invoker_mac(fields: &Provenance<'_>, invoker: Invoker) -> [u8; 32] {
    mac_over(&MASTER, org(), &message_v2(fields, invoker))
}

/// An outbox row with `fields`' values and the invoker columns given.
fn row_with(
    invoker: Option<&str>,
    token_id: Option<Uuid>,
    mac: Option<Vec<u8>>,
) -> oxy_oltp::events::OutboxRow {
    let f = fields();
    oxy_oltp::events::OutboxRow {
        id: f.id,
        writer_role: f.writer_role.to_string(),
        environment: f.environment.to_string(),
        name: "statement.uploaded".to_string(),
        key: f.key.to_string(),
        data_bytes: 64,
        schema_revision: f.schema_revision,
        actor: f.actor.map(str::to_string),
        causation_id: f.causation_id,
        root_id: f.root_id,
        depth: f.depth,
        invocation_id: f.invocation_id,
        delay_seconds: f.delay_seconds,
        created_at: chrono::Utc::now(),
        provenance: Some(sign_with(&MASTER, org(), &f).to_vec()),
        invoker: invoker.map(str::to_string),
        invoker_token_id: token_id,
        invoker_provenance: mac,
    }
}

/// The host's invoker MAC verifies for the row and invoker it signed, and a
/// row carrying it yields that invoker.
#[test]
fn the_hosts_invoker_mac_verifies() {
    for invoker in [Invoker::Session, token()] {
        let mac = invoker_mac(&fields(), invoker);
        let row = row_with(Some(invoker.kind()), invoker.token_id(), Some(mac.to_vec()));
        assert_eq!(
            verified_invoker_with(&MASTER, org(), &fields(), &row),
            Some(invoker),
            "{invoker:?}"
        );
    }
}

/// The invoker MAC binds the whole row: a change to any v1 field, or to the
/// invoker's kind or token, fails it.
#[test]
fn the_invoker_mac_binds_the_row_and_the_invoker() {
    let signed = invoker_mac(&fields(), token());
    for (name, changed) in each_field_changed() {
        assert!(
            !verify_over(
                &MASTER,
                org(),
                &message_v2(&changed, token()),
                Some(&signed)
            ),
            "{name} changed, yet the invoker MAC verified"
        );
    }
    for other in [Invoker::Session, Invoker::Token(Uuid::from_u128(0x71))] {
        assert!(
            !verify_over(&MASTER, org(), &message_v2(&fields(), other), Some(&signed)),
            "{other:?} passed for {:?}",
            token()
        );
    }
}

/// The two domains never pass for each other: the row's v1 MAC is not an
/// invoker MAC (so a v1 row, whatever its columns say, has no invoker), and
/// an invoker MAC is not a row's MAC.
#[test]
fn a_v1_mac_is_never_an_invoker_mac() {
    let v1 = sign_with(&MASTER, org(), &fields());
    let row = row_with(Some("session"), None, Some(v1.to_vec()));
    assert_eq!(verified_invoker_with(&MASTER, org(), &fields(), &row), None);
    let v2 = invoker_mac(&fields(), Invoker::Session);
    assert!(!verify_with(&MASTER, org(), &fields(), Some(&v2)));
    assert_ne!(v1, v2);
}

/// A row with no invoker, or one whose invoker MAC is missing, short or
/// another org's, yields none: it is the host's row, run as the system.
#[test]
fn a_row_without_a_signed_invoker_has_none() {
    assert_eq!(
        verified_invoker_with(&MASTER, org(), &fields(), &row_with(None, None, None)),
        None
    );
    let good = invoker_mac(&fields(), Invoker::Session).to_vec();
    let foreign = mac_over(
        &MASTER,
        Uuid::from_u128(0x12),
        &message_v2(&fields(), Invoker::Session),
    )
    .to_vec();
    for mac in [None, Some(good[..31].to_vec()), Some(foreign)] {
        let row = row_with(Some("session"), None, mac.clone());
        assert_eq!(
            verified_invoker_with(&MASTER, org(), &fields(), &row),
            None,
            "{mac:?}"
        );
    }
}

/// Only a well-formed pair of columns names an invoker.
#[test]
fn only_well_formed_columns_name_an_invoker() {
    let id = Uuid::from_u128(0x70);
    assert_eq!(
        Invoker::from_columns(Some("session"), None),
        Some(Invoker::Session)
    );
    assert_eq!(
        Invoker::from_columns(Some("token"), Some(id)),
        Some(Invoker::Token(id))
    );
    for (kind, token) in [
        (None, None),
        (None, Some(id)),
        (Some("token"), None),
        (Some("session"), Some(id)),
        (Some("system"), None),
        (Some("Session"), None),
    ] {
        assert_eq!(
            Invoker::from_columns(kind, token),
            None,
            "{kind:?} {token:?}"
        );
    }
}

/// A session and a token are told apart in the encoding, whatever the
/// token's id.
#[test]
fn a_session_never_encodes_as_a_token() {
    assert_ne!(
        message_v2(&fields(), Invoker::Session),
        message_v2(&fields(), Invoker::Token(Uuid::nil()))
    );
}
