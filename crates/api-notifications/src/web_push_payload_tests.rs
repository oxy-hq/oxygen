use std::collections::BTreeSet;

use uuid::Uuid;

use super::*;
use crate::native_push::tests::{row, signal};

/// A browser's subscription, minted per run: its key pair, its auth secret,
/// and the keys as `PushSubscription.toJSON()` would hand them over.
fn subscriber() -> (Box<dyn ece::LocalKeyPair>, [u8; 16], WebPushKeys) {
    let (pair, auth) = ece::generate_keypair_and_auth_secret().expect("keygen");
    let keys = WebPushKeys {
        p256dh: URL_SAFE_NO_PAD.encode(pair.pub_as_raw().expect("public key")),
        auth: URL_SAFE_NO_PAD.encode(auth),
    };
    (pair, auth, keys)
}

/// What the browser reads: decrypted with the subscription's private key.
fn open(pair: &dyn ece::LocalKeyPair, auth: &[u8], body: &[u8]) -> Payload {
    let components = pair.raw_components().expect("components");
    let plaintext = ece::decrypt(&components, auth, body).expect("the browser can decrypt it");
    serde_json::from_slice(&plaintext).expect("a v1 payload")
}

fn app_signal() -> PushSignal {
    PushSignal {
        app_id: Some(Uuid::from_u128(7)),
        app_name: "Store Ops".into(),
        label: "Temperature alarm".into(),
        ..signal("temp_alarm")
    }
}

#[test]
fn a_payload_names_the_app_and_the_kind_and_lands_on_that_kind() {
    let s = app_signal();
    assert_eq!(
        Payload::new(&s),
        Payload {
            v: 1,
            id: s.notification_id.to_string(),
            app_name: "Store Ops".into(),
            kind_label: "Temperature alarm".into(),
            href: "/notifications?source=00000000-0000-0000-0000-000000000007&kind=temp_alarm"
                .into(),
            thread: None,
        }
    );
}

#[test]
fn a_platform_notice_is_from_oxygen() {
    let s = PushSignal {
        label: "Run failed".into(),
        ..signal("oxy.run_failed")
    };
    let p = Payload::new(&s);
    assert_eq!(p.app_name, "Oxygen");
    assert_eq!(p.href, "/notifications?source=oxygen&kind=oxy.run_failed");
    // A blank name is no name.
    let blank = PushSignal {
        app_name: "  ".into(),
        ..app_signal()
    };
    assert_eq!(Payload::new(&blank).app_name, "Oxygen");
}

/// The quiet-hours summary is Oxygen's, counts what was held, and opens the
/// whole page: its kind has no inbox rows to narrow to.
#[test]
fn the_quiet_hours_summary_opens_the_notifications_page() {
    use crate::settings::summary::{SUMMARY_KIND, label};
    let s = PushSignal {
        label: label(3),
        ..signal(SUMMARY_KIND)
    };
    let p = Payload::new(&s);
    assert_eq!(p.app_name, "Oxygen");
    assert_eq!(p.kind_label, "3 notifications while quiet hours were on");
    assert_eq!(p.href, "/notifications");
    assert_eq!(p.thread, None);
}

#[test]
fn a_thread_lands_on_the_thread_and_tags_the_push() {
    let s = PushSignal {
        thread: Some("ab12".into()),
        ..app_signal()
    };
    let p = Payload::new(&s);
    assert_eq!(
        p.href,
        "/notifications?source=00000000-0000-0000-0000-000000000007&thread=ab12"
    );
    assert_eq!(p.thread.as_deref(), Some("ab12"));
}

/// The rule the payload exists under: ids and config, never what the
/// notification says. `PushSignal` holds no title, body or link — they are in
/// the org's store — so this pins the payload's SHAPE: exactly these keys, and
/// none of the ones content would arrive under.
#[test]
fn the_payload_carries_ids_and_config_and_never_a_title_or_body() {
    let s = PushSignal {
        thread: Some("ab12".into()),
        ..app_signal()
    };
    let value = serde_json::to_value(Payload::new(&s)).expect("serializes");
    let keys: BTreeSet<&str> = value
        .as_object()
        .expect("an object")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        BTreeSet::from(["v", "id", "app_name", "kind_label", "href", "thread"])
    );
    for content in ["title", "body", "link", "message", "text", "email_html"] {
        assert!(
            value.get(content).is_none(),
            "the payload carries `{content}`"
        );
    }
    // No thread, no key — not a null the worker has to read past.
    let bare = serde_json::to_value(Payload::new(&app_signal())).expect("serializes");
    assert!(bare.get("thread").is_none());
}

/// The same rule end to end: a recipient's row whose title, body and link
/// carry words (legacy rows can) goes through `PushSignal::new` — the only way
/// delivery builds a signal — and none of those words reach the payload.
#[test]
fn a_rows_title_body_and_link_never_reach_the_payload() {
    use crate::catalog::model::Level;
    use crate::catalog::source::KindInfo;

    let row = entity::notifications::Model {
        id: Uuid::new_v4(),
        org_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        kind: "temp_alarm".into(),
        title: "Walk-in cooler is at 9C".into(),
        body: Some("Move the salmon to freezer 2".into()),
        link: Some("/units/walk-in-3".into()),
        subject_kind: None,
        subject_id: None,
        created_at: chrono::Utc::now().fixed_offset(),
        read_at: None,
        message_id: None,
        app_id: Some(Uuid::from_u128(7)),
        thread_key: None,
    };
    let info = KindInfo {
        kind: "temp_alarm".into(),
        label: "Temperature alarm".into(),
        description: None,
        level: Level::Critical,
        category: "operations".into(),
        category_labels: vec!["Operations".into()],
        actions: vec![],
        channels: vec!["push"],
        mutable: false,
    };
    let payload = Payload::new(&PushSignal::new(&row, &info, "Store Ops"));
    let json = serde_json::to_string(&payload).expect("serializes");
    for words in ["cooler", "salmon", "walk-in-3"] {
        assert!(!json.contains(words), "`{words}` reached the push: {json}");
    }
    assert_eq!(payload.app_name, "Store Ops");
    assert_eq!(payload.kind_label, "Temperature alarm");
    assert_eq!(payload.id, row.id.to_string());
}

#[test]
fn it_round_trips_through_rfc_8291_encryption() {
    let (pair, auth, keys) = subscriber();
    let sub = SubscriptionKeys::parse(&keys).expect("a browser's keys");
    let payload = Payload::new(&PushSignal {
        thread: Some("ab12".into()),
        ..app_signal()
    });

    let body = encrypt(&sub, &payload).expect("encrypts");
    assert_eq!(open(&*pair, &auth, &body), payload);

    // RFC 8188 §2.1 header, as RFC 8291 §4 fixes it: salt(16) rs(4) idlen(1)
    // keyid, where keyid is the SENDER's ephemeral public key (65 bytes, 0x04)
    // — not the subscriber's, which would mean no key agreement happened.
    let rs = u32::from_be_bytes(body[16..20].try_into().expect("4 bytes"));
    assert_eq!(rs, 4096);
    assert_eq!(body[20], 65);
    assert_eq!(body[21], 0x04);
    assert_ne!(
        &body[21..86],
        pair.pub_as_raw().expect("public key").as_slice()
    );
    assert!(
        !body.windows(9).any(|w| w == b"Store Ops"),
        "the app's name is readable in what the push service relays"
    );

    // A fresh key and salt per message.
    assert_ne!(encrypt(&sub, &payload).expect("encrypts"), body);
}

#[test]
fn only_the_subscriptions_auth_secret_opens_it() {
    let (pair, _, keys) = subscriber();
    let body = encrypt(
        &SubscriptionKeys::parse(&keys).expect("keys"),
        &Payload::new(&app_signal()),
    )
    .expect("encrypts");
    let components = pair.raw_components().expect("components");
    assert!(ece::decrypt(&components, &[0u8; 16], &body).is_err());
}

#[test]
fn long_names_are_clipped_and_the_body_stays_under_the_push_limit() {
    let s = PushSignal {
        app_name: "S".repeat(10_000),
        label: "é".repeat(500),
        ..app_signal()
    };
    let p = Payload::new(&s);
    assert_eq!(
        p.app_name.chars().count(),
        MAX_TEXT_CHARS + 1,
        "clipped, with an ellipsis"
    );
    assert!(p.app_name.ends_with('…'));
    assert_eq!(p.kind_label.chars().count(), MAX_TEXT_CHARS + 1);
    let (_, _, keys) = subscriber();
    let body = encrypt(&SubscriptionKeys::parse(&keys).expect("keys"), &p).expect("fits");
    assert!(body.len() <= MAX_BODY_BYTES);
}

#[test]
fn subscription_keys_are_checked_by_shape() {
    let (_, _, keys) = subscriber();
    let parsed = SubscriptionKeys::parse(&keys).expect("a browser's keys");
    assert_eq!(parsed.to_stored(), (keys.p256dh.clone(), keys.auth.clone()));
    // Padding is tolerated.
    let padded = WebPushKeys {
        auth: format!("{}==", keys.auth),
        ..keys.clone()
    };
    assert_eq!(SubscriptionKeys::parse(&padded), Ok(parsed));

    let compressed = {
        let mut point = URL_SAFE_NO_PAD.decode(&keys.p256dh).expect("fixture");
        point[0] = 0x02;
        URL_SAFE_NO_PAD.encode(point)
    };
    for bad in [
        WebPushKeys {
            p256dh: "AAAA".into(),
            ..keys.clone()
        },
        WebPushKeys {
            p256dh: compressed,
            ..keys.clone()
        },
        WebPushKeys {
            auth: URL_SAFE_NO_PAD.encode([0u8; 15]),
            ..keys.clone()
        },
        WebPushKeys {
            auth: "not base64!".into(),
            ..keys.clone()
        },
        // the standard alphabet, not base64url
        WebPushKeys {
            auth: "+/+/+/+/+/+/+/+/+/+/+w".into(),
            ..keys.clone()
        },
    ] {
        assert!(SubscriptionKeys::parse(&bad).is_err(), "accepted {bad:?}");
    }
}

#[test]
fn a_rows_keys_are_both_or_neither() {
    let (_, _, keys) = subscriber();
    let bare = row("web", "https://push.example/a");
    assert!(
        SubscriptionKeys::of_row(&bare).is_none(),
        "no keys: a bodiless wake"
    );

    let with = device_tokens::Model {
        web_push_p256dh: Some(keys.p256dh.clone()),
        web_push_auth: Some(keys.auth.clone()),
        ..bare.clone()
    };
    assert!(matches!(SubscriptionKeys::of_row(&with), Some(Ok(_))));

    let half = device_tokens::Model {
        web_push_auth: None,
        ..with
    };
    assert!(matches!(SubscriptionKeys::of_row(&half), Some(Err(_))));
}
