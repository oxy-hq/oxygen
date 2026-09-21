//! DB-backed tests for frontline PIN credentials
//! ([`oxy_auth::frontline`]).
//!
//! These exist because the three properties that make a 4-digit PIN a
//! credential all live in the interaction between Rust and Postgres, and none
//! of them is observable from a unit test: the attempt counter has to survive
//! concurrency, the lockout has to expire cleanly, and a suspended worker has
//! to keep burning attempts. Two of the three were wrong when the module was
//! first written, and only a real database showed it.
//!
//! Own database per test through [`common::fresh_db`], so these sit in the
//! `db-per-test` nextest group with the other database-backed platform cases.
//!
//! Run with:
//! `cargo nextest run -p oxy-app --test platform -E 'test(frontline_pin)'`

use chrono::{Duration, Utc};
use entity::{organizations, user_credentials};
use oxy_auth::frontline::{self, PinPolicy, PinVerdict};
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set,
};
use uuid::Uuid;

/// A short-lockout policy so a test never waits on wall-clock minutes — the
/// reason `PinPolicy` is a parameter rather than a set of constants.
fn policy() -> PinPolicy {
    PinPolicy {
        max_attempts: 3,
        lockout_minutes: 15,
        ..PinPolicy::default()
    }
}

async fn setup_db() -> DatabaseConnection {
    let (db, _url) = crate::common::fresh_db(crate::common::Schema::Central).await;
    db
}

async fn seed_org(db: &DatabaseConnection) -> Uuid {
    let id = Uuid::new_v4();
    organizations::ActiveModel {
        id: ActiveValue::Set(id),
        name: ActiveValue::Set(format!("frontline-{}", id.simple())),
        slug: ActiveValue::Set(format!("frontline-{}", id.simple())),
        ..Default::default()
    }
    .insert(db)
    .await
    .expect("seed org");
    id
}

async fn credential(db: &DatabaseConnection, org_id: Uuid) -> user_credentials::Model {
    user_credentials::Entity::find()
        .filter(user_credentials::Column::OrgId.eq(Some(org_id)))
        .one(db)
        .await
        .expect("load credential")
        .expect("credential exists")
}

#[tokio::test]
async fn an_enrolled_worker_signs_in_and_a_wrong_pin_does_not() {
    let db = setup_db().await;
    let org = seed_org(&db).await;
    let user_id = frontline::enroll_worker(&db, org, "Maria S.", "maria.s", "4821", policy())
        .await
        .expect("enrol");

    let ok = frontline::verify_pin(&db, org, "maria.s", "4821", policy())
        .await
        .expect("verify");
    assert_eq!(ok, PinVerdict::Ok { user_id });

    // The enrolled user has no email — the whole point of the feature.
    let user = entity::users::Entity::find_by_id(user_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.email, None);
    assert_eq!(user.name, "Maria S.");

    assert!(matches!(
        frontline::verify_pin(&db, org, "maria.s", "4822", policy())
            .await
            .expect("verify"),
        PinVerdict::WrongPin { .. }
    ));
}

/// A right PIN the caller does not admit costs what a wrong PIN costs: one
/// charged attempt, no `last_used_at` stamp, no reset of the counter — and a
/// wrong PIN stays a wrong PIN whoever is asking.
#[tokio::test]
async fn a_right_pin_the_caller_does_not_admit_is_charged_like_a_wrong_one() {
    let db = setup_db().await;
    let org = seed_org(&db).await;
    let p = policy();
    let user_id = frontline::enroll_worker(&db, org, "Maria S.", "maria.s", "4821", p)
        .await
        .expect("enrol");

    let mut asked = None;
    let refused = frontline::verify_pin_admitting(&db, org, "maria.s", "4821", p, |u| {
        asked = Some(u);
        false
    })
    .await
    .expect("verify");
    assert_eq!(refused, PinVerdict::NotAdmitted);
    assert_eq!(
        asked,
        Some(user_id),
        "the rule is asked about the worker the identifier names"
    );
    let cred = credential(&db, org).await;
    assert_eq!(cred.failed_attempts, 1, "refused must be charged");
    assert_eq!(
        cred.last_used_at, None,
        "refused must not be stamped as used"
    );

    assert!(matches!(
        frontline::verify_pin_admitting(&db, org, "maria.s", "0000", p, |_| false)
            .await
            .expect("verify"),
        PinVerdict::WrongPin { .. }
    ));
    assert_eq!(credential(&db, org).await.failed_attempts, 2);

    // Admitted, the same PIN signs in — so the refusal above was the rule.
    assert_eq!(
        frontline::verify_pin_admitting(&db, org, "maria.s", "4821", p, |_| true)
            .await
            .expect("verify"),
        PinVerdict::Ok { user_id }
    );
}

#[tokio::test]
async fn concurrent_wrong_pins_each_count_against_the_budget() {
    // THE regression test for the lost update. The original implementation read
    // `failed_attempts`, added one in Rust, and wrote the absolute value back —
    // so N overlapping guesses all read `k` and all wrote `k + 1`, and N
    // guesses cost one attempt. The kiosk endpoint is unauthenticated by
    // construction, so firing concurrently instead of serially is free.
    let db = setup_db().await;
    let org = seed_org(&db).await;
    // A ceiling high enough that none of these attempts locks the credential —
    // this test is about the COUNT, not the lockout.
    let p = PinPolicy {
        max_attempts: 50,
        ..policy()
    };
    frontline::enroll_worker(&db, org, "Maria S.", "maria.s", "4821", p)
        .await
        .expect("enrol");

    let attempts = 8;
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..attempts {
        let db = db.clone();
        set.spawn(async move {
            frontline::verify_pin(&db, org, "maria.s", "0000", p)
                .await
                .expect("verify")
        });
    }
    while let Some(r) = set.join_next().await {
        let v = r.expect("task");
        assert!(!v.is_ok(), "a wrong PIN must never verify");
    }

    let cred = credential(&db, org).await;
    assert_eq!(
        cred.failed_attempts, attempts,
        "every concurrent guess must be charged; a lost update here is a \
         materially larger brute-force budget"
    );
}

#[tokio::test]
async fn the_credential_locks_and_the_lock_refuses_without_checking_the_secret() {
    let db = setup_db().await;
    let org = seed_org(&db).await;
    let p = policy(); // max_attempts = 3
    frontline::enroll_worker(&db, org, "Maria S.", "maria.s", "4821", p)
        .await
        .expect("enrol");

    for _ in 0..p.max_attempts {
        frontline::verify_pin(&db, org, "maria.s", "0000", p)
            .await
            .expect("verify");
    }

    let cred = credential(&db, org).await;
    assert!(
        cred.locked_until.is_some(),
        "the ceiling must arm the timer"
    );

    // The RIGHT pin is now refused — the lock is checked before the secret.
    assert_eq!(
        frontline::verify_pin(&db, org, "maria.s", "4821", p)
            .await
            .expect("verify"),
        PinVerdict::LockedOut
    );
}

#[tokio::test]
async fn a_lapsed_lockout_restores_the_full_budget() {
    // Regression: `failed_attempts` used to survive the window, so once a
    // worker had locked out, the counter stayed at the ceiling and the NEXT
    // single mistyped digit re-locked for another full window — permanently,
    // one typo away, on a tablet somebody is using mid-shift.
    let db = setup_db().await;
    let org = seed_org(&db).await;
    let p = policy(); // max_attempts = 3
    frontline::enroll_worker(&db, org, "Maria S.", "maria.s", "4821", p)
        .await
        .expect("enrol");

    for _ in 0..p.max_attempts {
        frontline::verify_pin(&db, org, "maria.s", "0000", p)
            .await
            .expect("verify");
    }

    // Backdate the lockout rather than sleeping through it.
    let cred = credential(&db, org).await;
    let mut m: user_credentials::ActiveModel = cred.into();
    m.locked_until = Set(Some((Utc::now() - Duration::minutes(1)).into()));
    m.update(&db).await.expect("backdate lockout");

    // One wrong PIN after the window: this must be attempt 1 of a fresh
    // budget, NOT attempt 4 that re-locks immediately.
    let verdict = frontline::verify_pin(&db, org, "maria.s", "0000", p)
        .await
        .expect("verify");
    assert_eq!(
        verdict,
        PinVerdict::WrongPin {
            attempts_remaining: p.max_attempts - 1
        },
        "a lapsed window must restore the budget, not leave the worker one typo \
         from another lockout"
    );
    let cred = credential(&db, org).await;
    assert_eq!(cred.failed_attempts, 1);
    assert!(
        cred.locked_until.is_none(),
        "the lapsed timer must be cleared"
    );

    // And the right PIN works again.
    assert!(
        frontline::verify_pin(&db, org, "maria.s", "4821", p)
            .await
            .expect("verify")
            .is_ok()
    );
}

#[tokio::test]
async fn a_suspended_worker_still_burns_attempts_and_reads_as_unknown() {
    // Folding `status = 'active'` into the credential lookup would make a
    // suspended worker MISS, return NoSuchWorker, and never lock — unlimited
    // guesses against exactly the accounts nobody is watching.
    let db = setup_db().await;
    let org = seed_org(&db).await;
    let p = policy();
    let user_id = frontline::enroll_worker(&db, org, "Gone A.", "gone.a", "4821", p)
        .await
        .expect("enrol");

    let mut standing: entity::org_frontline_members::ActiveModel =
        entity::org_frontline_members::Entity::find_by_id((org, user_id))
            .one(&db)
            .await
            .unwrap()
            .unwrap()
            .into();
    standing.status = Set("suspended".to_string());
    standing.update(&db).await.expect("suspend");

    // The CORRECT pin, from a suspended worker: answered as if they do not
    // exist, so the tablet learns nothing about whether the PIN was right.
    assert_eq!(
        frontline::verify_pin(&db, org, "gone.a", "4821", p)
            .await
            .expect("verify"),
        PinVerdict::NoSuchWorker
    );
    assert_eq!(
        credential(&db, org).await.failed_attempts,
        1,
        "a suspended worker's attempts must still be charged"
    );

    assert!(
        !frontline::is_active_frontline(&db, org, user_id)
            .await
            .expect("standing")
    );
}

#[tokio::test]
async fn an_unknown_identifier_and_a_malformed_pin_are_both_refused() {
    let db = setup_db().await;
    let org = seed_org(&db).await;
    let p = policy();

    assert_eq!(
        frontline::verify_pin(&db, org, "nobody", "4821", p)
            .await
            .expect("verify"),
        PinVerdict::NoSuchWorker
    );
    assert_eq!(
        frontline::verify_pin(&db, org, "nobody", "abc", p)
            .await
            .expect("verify"),
        PinVerdict::Malformed,
        "a malformed PIN is rejected before any database work"
    );
}

#[tokio::test]
async fn a_pin_is_scoped_to_one_org() {
    // The first of the three things that make a 4-digit PIN a credential. Two
    // orgs can hand out the same login name and the same PIN without either
    // reaching the other.
    let db = setup_db().await;
    let org_a = seed_org(&db).await;
    let org_b = seed_org(&db).await;
    let p = policy();

    let a = frontline::enroll_worker(&db, org_a, "Maria S.", "maria.s", "4821", p)
        .await
        .expect("enrol a");
    let b = frontline::enroll_worker(&db, org_b, "Maria S.", "maria.s", "4821", p)
        .await
        .expect("enrol b");
    assert_ne!(a, b, "same name and PIN, two different people");

    assert_eq!(
        frontline::verify_pin(&db, org_a, "maria.s", "4821", p)
            .await
            .expect("verify"),
        PinVerdict::Ok { user_id: a }
    );
    assert_eq!(
        frontline::verify_pin(&db, org_b, "maria.s", "4821", p)
            .await
            .expect("verify"),
        PinVerdict::Ok { user_id: b }
    );
}

/// Suspension takes away the sign-in and leaves the history.
///
/// The writer this pins is the one the model never had: `status` has modelled
/// `suspended` since the schema landed and nothing wrote it, so a worker could
/// be enrolled and never un-enrolled. I found that by enrolling a test worker
/// into a demo org and having no way to remove them.
///
/// What makes it worth a real database rather than a unit test is that ONE
/// column change is supposed to produce three behaviours — the login refuses,
/// it refuses in a particular way, and the user row survives — and none of those
/// is visible from the writer itself.
#[tokio::test]
async fn a_suspended_worker_cannot_sign_in_and_is_not_deleted() {
    let db = setup_db().await;
    let org = seed_org(&db).await;
    let user_id = frontline::enroll_worker(&db, org, "Gone A.", "gone.a", "4821", policy())
        .await
        .expect("enrol");

    // Signs in before, or the rest of this proves nothing.
    assert_eq!(
        frontline::verify_pin(&db, org, "gone.a", "4821", policy())
            .await
            .expect("verify"),
        PinVerdict::Ok { user_id }
    );

    assert!(
        frontline::set_worker_standing(&db, org, user_id, false)
            .await
            .expect("suspend"),
        "suspending an active worker must report that the row moved"
    );

    // The RIGHT PIN, and it must not work — and must not admit it is right.
    // A suspended worker is a former employee; confirming their PIN is still
    // correct is exactly the wrong thing to tell whoever holds the tablet.
    assert_eq!(
        frontline::verify_pin(&db, org, "gone.a", "4821", policy())
            .await
            .expect("verify"),
        PinVerdict::NoSuchWorker,
        "a suspended worker must be indistinguishable from one who never existed"
    );

    // The person is still there. Their submissions and findings point at this
    // id, and losing the row would take the record of who did the work along
    // with the ability to do it.
    assert!(
        entity::users::Entity::find_by_id(user_id)
            .one(&db)
            .await
            .expect("query")
            .is_some(),
        "suspension must not delete the person"
    );
    // And so is the credential — reinstating is a second call here, not a
    // re-enrolment that would make the worker learn a new PIN.
    assert!(
        entity::user_credentials::Entity::find()
            .filter(entity::user_credentials::Column::UserId.eq(user_id))
            .one(&db)
            .await
            .expect("query")
            .is_some(),
        "suspension must not drop the credential"
    );

    // Idempotent: asking for the state it is already in is not a conflict.
    assert!(
        !frontline::set_worker_standing(&db, org, user_id, false)
            .await
            .expect("suspend twice"),
        "a no-op must report that nothing moved, not fail"
    );

    // Burn the whole budget while they are gone.
    //
    // `verify_pin` charges a failed attempt against a suspended worker before
    // answering `NoSuchWorker` — on purpose, so the credential still locks
    // rather than offering unlimited guesses at exactly the accounts nobody is
    // watching. The consequence is a lockout accruing on a credential nobody is
    // supposed to be using, and the lockout check runs BEFORE the standing
    // check.
    //
    // The first version of this test made ONE suspended attempt against a budget
    // of three, so it never reached the state it was written to be safe in: a
    // worker coming back to `LockedOut` on the correct PIN.
    for _ in 0..policy().max_attempts {
        let _ = frontline::verify_pin(&db, org, "gone.a", "4821", policy()).await;
    }
    let locked = credential(&db, org).await;
    assert!(
        locked.locked_until.is_some(),
        "the budget must actually be spendable while suspended, or this proves nothing"
    );

    // Reinstating restores the ORIGINAL pin, which is the point of not deleting
    // the credential — and it must clear the lockout suspension caused, or the
    // "restores the original pin" promise means "in fifteen minutes".
    assert!(
        frontline::set_worker_standing(&db, org, user_id, true)
            .await
            .expect("reinstate")
    );
    let cleared = credential(&db, org).await;
    assert!(
        cleared.locked_until.is_none() && cleared.failed_attempts == 0,
        "reinstating must clear the lockout its own suspension caused"
    );
    assert_eq!(
        frontline::verify_pin(&db, org, "gone.a", "4821", policy())
            .await
            .expect("verify"),
        PinVerdict::Ok { user_id },
        "reinstating must not require the worker to learn a new PIN"
    );

    // Somebody else's worker is not found rather than refused, so an admin
    // cannot use this route to discover that a user id is on another tenant's
    // roster.
    let other_org = seed_org(&db).await;
    assert!(
        frontline::set_worker_standing(&db, other_org, user_id, false)
            .await
            .is_err(),
        "a worker must not be reachable through another org"
    );
}

/// A forgotten PIN is re-issued at the counter, and the lockout forgetting it
/// produced does not outlive the reset.
#[tokio::test]
async fn a_reset_pin_replaces_the_old_one_and_clears_a_lockout() {
    let db = setup_db().await;
    let org = seed_org(&db).await;
    let user_id = frontline::enroll_worker(&db, org, "Maria S.", "maria.s", "4821", policy())
        .await
        .expect("enrol");

    // Forget it: burn every attempt the policy allows, then even the right
    // PIN is refused — the lockout is real.
    for _ in 0..policy().max_attempts {
        let _ = frontline::verify_pin(&db, org, "maria.s", "0000", policy()).await;
    }
    assert!(
        !matches!(
            frontline::verify_pin(&db, org, "maria.s", "4821", policy())
                .await
                .expect("verify"),
            PinVerdict::Ok { .. }
        ),
        "the fixture must actually be locked out for the reset to prove anything"
    );

    frontline::reset_pin(&db, org, user_id, "9753", policy())
        .await
        .expect("reset");

    // The new PIN works at once — the lockout went with the old one.
    assert_eq!(
        frontline::verify_pin(&db, org, "maria.s", "9753", policy())
            .await
            .expect("verify"),
        PinVerdict::Ok { user_id }
    );
    // And the old one is gone, not merely joined by the new.
    assert!(!matches!(
        frontline::verify_pin(&db, org, "maria.s", "4821", policy())
            .await
            .expect("verify"),
        PinVerdict::Ok { .. }
    ));

    // The policy holds on a reset as on enrolment; an unknown worker is named.
    assert!(matches!(
        frontline::reset_pin(&db, org, user_id, "1", policy()).await,
        Err(oxy_shared::errors::OxyError::ValidationError(_))
    ));
    match frontline::reset_pin(&db, org, Uuid::new_v4(), "9753", policy()).await {
        Err(oxy_shared::errors::OxyError::ValidationError(msg)) => {
            assert_eq!(msg, frontline::WORKER_NOT_FOUND)
        }
        other => panic!("expected WORKER_NOT_FOUND, got {other:?}"),
    }
}
