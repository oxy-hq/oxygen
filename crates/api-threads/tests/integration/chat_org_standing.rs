//! A `chat_channel_members` row is not, on its own, permission to read a channel.
//!
//! Its foreign keys reach `chat_channels` and `users` only, so nothing removes
//! it when somebody leaves the org. Both chat gates therefore have to re-check
//! org standing at read time, and this is the case that proves they do:
//! membership row present, org membership gone, access refused.
//!
//! Moved from `oxy-app`'s `platform` group with the chat handlers
//! (2026-11-16). It uses `common::fresh_db`, its own database per test, which
//! `.config/nextest.toml` routes to `db-per-test` for this crate's
//! integration group.
//!
//! Run with:
//! `cargo nextest run -p oxy-api-threads --test integration -E 'test(chat_org_standing)'`

use chrono::Utc;
use entity::{
    chat_channel_members, chat_channels, org_frontline_members, org_members, organizations, users,
};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter,
};
use uuid::Uuid;

use oxy_api_threads::chat::handlers::{
    join_channel_for, member_channel, new_channel, unread_for, visible_channels,
};

use crate::common::{Schema, fresh_db};

struct Fixture {
    user: Uuid,
    org: Uuid,
    channel: Uuid,
}

/// One org, one member, one channel they belong to.
async fn seed(db: &DatabaseConnection) -> Fixture {
    let user = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(user),
        email: ActiveValue::Set(Some(format!("chat-{user}@example.com"))),
        name: ActiveValue::Set("Chat Test".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed user");

    let org = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(org),
        name: ActiveValue::Set("Chat Org".into()),
        slug: ActiveValue::Set(format!("chat-org-{org}")),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");

    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(user),
        role: ActiveValue::Set(org_members::OrgRole::Member),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org membership");

    let channel = Uuid::new_v4();
    chat_channels::ActiveModel {
        id: ActiveValue::Set(channel),
        org_id: ActiveValue::Set(org),
        kind: ActiveValue::Set("channel".into()),
        name: ActiveValue::Set(Some("ops".into())),
        topic: ActiveValue::Set(None),
        created_by: ActiveValue::Set(Some(user)),
        created_at: ActiveValue::Set(Utc::now().fixed_offset()),
        archived_at: ActiveValue::Set(None),
    }
    .insert(db)
    .await
    .expect("seed channel");

    chat_channel_members::ActiveModel {
        channel_id: ActiveValue::Set(channel),
        user_id: ActiveValue::Set(user),
        joined_at: ActiveValue::Set(Utc::now().fixed_offset()),
        last_read_at: ActiveValue::Set(None),
        muted: ActiveValue::Set(false),
    }
    .insert(db)
    .await
    .expect("seed channel membership");

    Fixture { user, org, channel }
}

/// A second user with real standing in an existing org.
async fn seed_member(db: &DatabaseConnection, org: Uuid, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(id),
        email: ActiveValue::Set(Some(format!("m-{id}@example.com"))),
        name: ActiveValue::Set(name.into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed member user");
    org_members::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        org_id: ActiveValue::Set(org),
        user_id: ActiveValue::Set(id),
        role: ActiveValue::Set(org_members::OrgRole::Member),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed member org row");
    id
}

async fn remove_from_org(db: &DatabaseConnection, user: Uuid, org: Uuid) {
    org_members::Entity::delete_many()
        .filter(org_members::Column::UserId.eq(user))
        .filter(org_members::Column::OrgId.eq(org))
        .exec(db)
        .await
        .expect("remove org membership");
}

async fn still_a_channel_member(db: &DatabaseConnection, f: &Fixture) -> bool {
    chat_channel_members::Entity::find_by_id((f.channel, f.user))
        .one(db)
        .await
        .expect("query channel membership")
        .is_some()
}

/// The gate on a single channel — reads, writes and the SSE stream all inherit
/// this one, so it is the load-bearing half.
#[tokio::test]
async fn a_removed_org_member_loses_the_channel() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let f = seed(&db).await;

    assert!(
        member_channel(&db, f.channel, f.user).await.is_some(),
        "fixture did not grant access in the first place"
    );

    remove_from_org(&db, f.user, f.org).await;

    assert!(
        still_a_channel_member(&db, &f).await,
        "the chat_channel_members row was deleted, so this test would pass for \
         the wrong reason — the whole point is that it survives"
    );
    assert!(
        member_channel(&db, f.channel, f.user).await.is_none(),
        "a removed org member still reaches the channel"
    );
}

/// The list gate. Separate from the one above because it was separately missing:
/// `member_channel` was fixed while `list_channels` went straight from
/// membership rows to summaries, so a removed member kept seeing the channel's
/// name, topic, member count and a live unread count.
#[tokio::test]
async fn a_removed_org_member_loses_the_channel_list() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let f = seed(&db).await;

    let before = visible_channels(&db, f.user).await.expect("list");
    assert_eq!(before.len(), 1, "fixture did not produce a visible channel");
    assert_eq!(before[0].1.id, f.channel);

    remove_from_org(&db, f.user, f.org).await;

    assert!(still_a_channel_member(&db, &f).await);
    assert!(
        visible_channels(&db, f.user)
            .await
            .expect("list")
            .is_empty(),
        "a removed org member still sees the channel in their list"
    );
}

/// A membership row for a channel in an org the user was never in must not grant
/// anything either — the gate is standing, not history.
#[tokio::test]
async fn a_membership_row_alone_grants_nothing() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let f = seed(&db).await;

    // A second user with a membership row but no org standing at all.
    let outsider = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(outsider),
        email: ActiveValue::Set(Some(format!("out-{outsider}@example.com"))),
        name: ActiveValue::Set("Outsider".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed outsider");
    chat_channel_members::ActiveModel {
        channel_id: ActiveValue::Set(f.channel),
        user_id: ActiveValue::Set(outsider),
        joined_at: ActiveValue::Set(Utc::now().fixed_offset()),
        last_read_at: ActiveValue::Set(None),
        muted: ActiveValue::Set(false),
    }
    .insert(&db)
    .await
    .expect("seed outsider membership");

    assert!(member_channel(&db, f.channel, outsider).await.is_none());
    assert!(
        visible_channels(&db, outsider)
            .await
            .expect("list")
            .is_empty()
    );
}

/// A frontline worker holds NO `org_members` row by design — they are enrolled
/// by PIN on a shared tablet — and they are the primary audience for these
/// channels.
///
/// This is the case the org-standing gate would have broken silently: checking
/// membership alone is correct for every office user and locks out exactly the
/// people chat exists for. It only became testable once
/// `org_frontline_members` landed on main.
#[tokio::test]
async fn an_active_frontline_worker_reaches_the_channel() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let f = seed(&db).await;

    // Same org, same channel, but standing comes from frontline enrolment only.
    let worker = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(worker),
        // No email: a frontline worker is a user who may not have a mailbox.
        email: ActiveValue::Set(None),
        name: ActiveValue::Set("Shift Lead".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed frontline user");
    org_frontline_members::ActiveModel {
        org_id: ActiveValue::Set(f.org),
        user_id: ActiveValue::Set(worker),
        status: ActiveValue::Set("active".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("enrol frontline worker");
    chat_channel_members::ActiveModel {
        channel_id: ActiveValue::Set(f.channel),
        user_id: ActiveValue::Set(worker),
        joined_at: ActiveValue::Set(Utc::now().fixed_offset()),
        last_read_at: ActiveValue::Set(None),
        muted: ActiveValue::Set(false),
    }
    .insert(&db)
    .await
    .expect("add worker to channel");

    assert!(
        org_members::Entity::find()
            .filter(org_members::Column::UserId.eq(worker))
            .one(&db)
            .await
            .expect("query")
            .is_none(),
        "fixture gave the worker an org_members row, so it proves nothing"
    );
    assert!(
        member_channel(&db, f.channel, worker).await.is_some(),
        "a frontline worker cannot reach the channel they were added to"
    );
    assert_eq!(
        visible_channels(&db, worker).await.expect("list").len(),
        1,
        "a frontline worker sees an empty channel list"
    );
}

/// Suspending a worker takes their chat access with it — one row, one effect.
#[tokio::test]
async fn a_suspended_frontline_worker_loses_the_channel() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let f = seed(&db).await;

    let worker = Uuid::new_v4();
    users::ActiveModel {
        id: ActiveValue::Set(worker),
        email: ActiveValue::Set(None),
        name: ActiveValue::Set("Suspended".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(false),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("seed user");
    org_frontline_members::ActiveModel {
        org_id: ActiveValue::Set(f.org),
        user_id: ActiveValue::Set(worker),
        status: ActiveValue::Set("suspended".into()),
        ..Default::default()
    }
    .insert(&db)
    .await
    .expect("enrol suspended");
    chat_channel_members::ActiveModel {
        channel_id: ActiveValue::Set(f.channel),
        user_id: ActiveValue::Set(worker),
        joined_at: ActiveValue::Set(Utc::now().fixed_offset()),
        last_read_at: ActiveValue::Set(None),
        muted: ActiveValue::Set(false),
    }
    .insert(&db)
    .await
    .expect("add to channel");

    assert!(member_channel(&db, f.channel, worker).await.is_none());
    assert!(
        visible_channels(&db, worker)
            .await
            .expect("list")
            .is_empty()
    );
}

// ── create / join ───────────────────────────────────────────────────────────

/// A created channel must be reachable by its creator. Membership is the read
/// gate, so a channel created without the creator in it is invisible to the
/// person who just made it — and, having no members, to everyone.
#[tokio::test]
async fn a_created_channel_is_reachable_by_its_creator() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let f = seed(&db).await;

    let id = new_channel(&db, f.user, f.org, "ops-standup", None, &[])
        .await
        .expect("create")
        .id;

    assert!(member_channel(&db, id, f.user).await.is_some());
    let visible = visible_channels(&db, f.user).await.expect("list");
    assert!(visible.iter().any(|(_, c)| c.id == id));
}

/// Seeded members join at creation, and the creator is not duplicated when
/// they name themselves — `chat_channel_members` is keyed on the pair.
#[tokio::test]
async fn seeded_members_join_and_the_creator_is_not_duplicated() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let f = seed(&db).await;
    let other = seed_member(&db, f.org, "Colleague").await;

    let id = new_channel(&db, f.user, f.org, "kitchen", None, &[other, f.user])
        .await
        .expect("create")
        .id;

    assert!(member_channel(&db, id, other).await.is_some());
    assert_eq!(
        chat_channel_members::Entity::find()
            .filter(chat_channel_members::Column::ChannelId.eq(id))
            .all(&db)
            .await
            .expect("count")
            .len(),
        2,
        "the creator was seeded twice or a member was dropped"
    );
}

/// The lesson the assignment graph learned: ids arrive in the body, and an
/// unchecked one seeds somebody from another tenant into a readable channel.
#[tokio::test]
async fn a_member_from_another_org_is_refused() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let f = seed(&db).await;
    let outsider = seed(&db).await; // its own org

    let err = new_channel(&db, f.user, f.org, "leaky", None, &[outsider.user]).await;
    assert!(err.is_err(), "a cross-tenant member was accepted");
    // And nothing was written — the whole create is one transaction.
    assert!(
        chat_channels::Entity::find()
            .filter(chat_channels::Column::OrgId.eq(f.org))
            .all(&db)
            .await
            .expect("query")
            .iter()
            .all(|c| c.id == f.channel),
        "a channel was left behind by a refused create"
    );
}

/// Creating in an org you have no standing in is refused.
#[tokio::test]
async fn creating_in_a_foreign_org_is_refused() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let mine = seed(&db).await;
    let theirs = seed(&db).await;

    assert!(
        new_channel(&db, mine.user, theirs.org, "trespass", None, &[])
            .await
            .is_err()
    );
}

/// Joining is what makes a named channel a channel rather than a DM, and a
/// double tap must not be an error.
#[tokio::test]
async fn joining_is_open_to_the_org_and_idempotent() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let f = seed(&db).await;
    let joiner = seed_member(&db, f.org, "Joiner").await;

    assert!(
        member_channel(&db, f.channel, joiner).await.is_none(),
        "fixture already had them in the channel"
    );
    join_channel_for(&db, joiner, f.channel)
        .await
        .expect("join");
    assert!(member_channel(&db, f.channel, joiner).await.is_some());

    join_channel_for(&db, joiner, f.channel)
        .await
        .expect("a second join is a double tap, not an error");
    assert_eq!(
        chat_channel_members::Entity::find()
            .filter(chat_channel_members::Column::ChannelId.eq(f.channel))
            .filter(chat_channel_members::Column::UserId.eq(joiner))
            .all(&db)
            .await
            .expect("count")
            .len(),
        1
    );
}

/// Someone with no standing in the org cannot join their way in.
#[tokio::test]
async fn joining_from_another_org_is_refused() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let f = seed(&db).await;
    let outsider = seed(&db).await;

    assert!(
        join_channel_for(&db, outsider.user, f.channel)
            .await
            .is_err()
    );
    assert!(
        member_channel(&db, f.channel, outsider.user)
            .await
            .is_none()
    );
}

/// Joining a busy channel must not mark its whole history unread.
///
/// `last_read_at` starts NULL, and counting every message when it is NULL means
/// joining a #general with 5,000 messages shows 5,000 unread — and, since the
/// list sorts busiest-first, pins that channel to the top of the joiner's list
/// until they open it. `joined_at` is the honest cutoff: unread means "since I
/// could have read it", not "since the channel began".
#[tokio::test]
async fn joining_does_not_mark_the_whole_history_unread() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let f = seed(&db).await;

    // Three messages that predate the join.
    for i in 0..3 {
        entity::chat_messages::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            channel_id: ActiveValue::Set(f.channel),
            author_id: ActiveValue::Set(Some(f.user)),
            body: ActiveValue::Set(format!("old {i}")),
            created_at: ActiveValue::NotSet,
            edited_at: ActiveValue::Set(None),
            deleted_at: ActiveValue::Set(None),
        }
        .insert(&db)
        .await
        .expect("seed message");
    }

    let joiner = seed_member(&db, f.org, "Late Joiner").await;
    join_channel_for(&db, joiner, f.channel)
        .await
        .expect("join");

    assert_eq!(
        unread_for(&db, joiner, f.channel).await.expect("unread"),
        0,
        "the joiner inherited the channel's whole history as unread"
    );

    // A message posted AFTER the join is unread, or the cutoff is too late.
    entity::chat_messages::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        channel_id: ActiveValue::Set(f.channel),
        author_id: ActiveValue::Set(Some(f.user)),
        body: ActiveValue::Set("after you joined".into()),
        created_at: ActiveValue::NotSet,
        edited_at: ActiveValue::Set(None),
        deleted_at: ActiveValue::Set(None),
    }
    .insert(&db)
    .await
    .expect("seed message");

    assert_eq!(
        unread_for(&db, joiner, f.channel).await.expect("unread"),
        1,
        "a message posted after the join did not count as unread"
    );
}

/// What the create answers must be what a later read answers.
///
/// The handler used to trim independently of `new_channel`, so `topic: "  "`
/// produced `{"topic": ""}` on create and `topic: null` on the next list — a
/// client caching a value the server never stored.
#[tokio::test]
async fn the_create_response_matches_what_was_stored() {
    let (db, _url) = fresh_db(Schema::Central).await;
    let f = seed(&db).await;

    let made = new_channel(&db, f.user, f.org, "  spaced  ", Some("   "), &[])
        .await
        .expect("create");

    assert_eq!(made.name, "spaced", "the name was not normalised once");
    assert_eq!(
        made.topic, None,
        "a whitespace topic answered as empty, not null"
    );

    let stored = entity::chat_channels::Entity::find_by_id(made.id)
        .one(&db)
        .await
        .expect("query")
        .expect("row");
    assert_eq!(stored.name.as_deref(), Some(made.name.as_str()));
    assert_eq!(
        stored.topic, made.topic,
        "create answered something else than it stored"
    );
}
