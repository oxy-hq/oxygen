//! One mailbox, one account — whatever capitals the address arrives in.
//!
//! A magic link lowercases what was typed; Google, GitHub and Okta hand over
//! what the provider holds. Matched exactly, the same person signing in two
//! ways became two accounts. Lookup now ignores case, new accounts are stored
//! lowercase — and the duplicates that already exist each keep answering to
//! their own spelling, so nobody is moved from one account to the other.

use chrono::{Duration, Utc};
use entity::prelude::Users;
use entity::users::{self, UserStatus};
use oxy::database::filters::UserQueryFilterExt;
use oxy_auth::types::Identity;
use oxy_auth::user::UserService;
use sea_orm::{ActiveModelTrait, ActiveValue, DatabaseConnection, EntityTrait};
use uuid::Uuid;

use super::fixture;

/// A user stored with exactly this address, created `age_days` ago.
async fn seed(db: &DatabaseConnection, email: &str, age_days: i64) -> users::Model {
    users::ActiveModel {
        id: ActiveValue::Set(Uuid::new_v4()),
        email: ActiveValue::Set(Some(email.to_string())),
        name: ActiveValue::Set("Jane".into()),
        picture: ActiveValue::Set(None),
        email_verified: ActiveValue::Set(true),
        magic_link_token: ActiveValue::Set(None),
        magic_link_token_expires_at: ActiveValue::Set(None),
        status: ActiveValue::Set(UserStatus::Active),
        created_at: ActiveValue::Set((Utc::now() - Duration::days(age_days)).fixed_offset()),
        last_login_at: ActiveValue::NotSet,
    }
    .insert(db)
    .await
    .expect("seed user")
}

async fn found(db: &DatabaseConnection, email: &str) -> Option<Uuid> {
    Users::find()
        .filter_by_email(email)
        .one(db)
        .await
        .unwrap()
        .map(|user| user.id)
}

#[tokio::test]
async fn an_account_made_with_capitals_is_found_by_any_spelling() {
    let fx = fixture().await;
    let tag = Uuid::new_v4().simple().to_string();
    // What GitHub or Google sign-in stored before addresses were lowercased.
    let stored = seed(&fx.db, &format!("Jane.Doe-{tag}@Example.com"), 1).await;

    for spelling in [
        format!("jane.doe-{tag}@example.com"),
        format!("JANE.DOE-{tag}@EXAMPLE.COM"),
        format!("Jane.Doe-{tag}@Example.com"),
    ] {
        assert_eq!(
            found(&fx.db, &spelling).await,
            Some(stored.id),
            "{spelling}"
        );
    }
    // The control: another mailbox is still another person.
    assert_eq!(
        found(&fx.db, &format!("jane.roe-{tag}@example.com")).await,
        None
    );
}

#[tokio::test]
async fn two_accounts_that_differ_only_by_case_each_keep_their_own_spelling() {
    let fx = fixture().await;
    let tag = Uuid::new_v4().simple().to_string();
    // The pair exact matching made: the provider's spelling first, then the
    // lowercase one a magic link created for the same person.
    let from_provider = seed(&fx.db, &format!("Jane-{tag}@Example.com"), 30).await;
    let from_magic_link = seed(&fx.db, &format!("jane-{tag}@example.com"), 2).await;

    // Each way in still lands where it always has.
    assert_eq!(
        found(&fx.db, &format!("Jane-{tag}@Example.com")).await,
        Some(from_provider.id)
    );
    assert_eq!(
        found(&fx.db, &format!("jane-{tag}@example.com")).await,
        Some(from_magic_link.id)
    );
    // A spelling neither row has: the older account, every time.
    assert_eq!(
        found(&fx.db, &format!("JANE-{tag}@EXAMPLE.COM")).await,
        Some(from_provider.id)
    );
}

#[tokio::test]
async fn a_new_account_is_stored_lowercase_and_made_once() {
    let _fx = fixture().await;
    let tag = Uuid::new_v4().simple().to_string();
    let identity = |email: String| Identity {
        user_id: None,
        picture: None,
        name: Some("New Person".into()),
        email,
    };

    let first = UserService::get_or_create_user(&identity(format!("New.Person-{tag}@Example.com")))
        .await
        .expect("create");
    assert_eq!(
        first.email.as_deref(),
        Some(format!("new.person-{tag}@example.com").as_str())
    );

    // The same mailbox in other capitals is the same account, not a second one.
    let again = UserService::get_or_create_user(&identity(format!("NEW.PERSON-{tag}@example.COM")))
        .await
        .expect("find");
    assert_eq!(again.id, first.id);
}
