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
