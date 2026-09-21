//! Frontline sign-in — proving who you are without an email address.
//!
//! The rest of this crate authenticates people who have a mailbox: a magic link
//! or an SSO provider hands back an [`Identity`](crate::types::Identity), and
//! `users.email` resolves it to a row. A restaurant's opening checklist has no
//! such person at the screen. It has one of 127 hourly staff on a shared
//! tablet, and the submission still has to say who did it.
//!
//! This module is the credential half of that: enrol a worker with a PIN,
//! verify a PIN, and refuse convincingly when it is wrong. It deliberately
//! stops short of minting a session — see **What this does not do** below.
//!
//! Design record: `internal-docs/frontline-identity.md`.
//!
//! # Why a 4-digit PIN is a credential here
//!
//! On its own it is 10,000 possibilities, which is not a credential. Three
//! things make it one, and removing any of them breaks the argument:
//!
//! 1. **It is scoped to one org**, not global. `user_credentials` is unique on
//!    `(kind, org_id, identifier)`, so an attacker must also know which org and
//!    which worker.
//! 2. **It is throttled, and the throttle survives a race.** [`verify_pin`]
//!    charges the attempt in the same statement that fails it, so two kiosks
//!    guessing in parallel cannot both read `failed_attempts = 4` and neither
//!    lock.
//! 3. **It buys a shift, not a session.** That boundary is the caller's to
//!    enforce, and it is why this module hands back a `user_id` rather than a
//!    token.
//!
//! # What this does not do, on purpose
//!
//! No session, no cookie, no JWT. A frontline session has open questions this
//! module cannot answer alone — whether it is bound to the enrolled device,
//! what closes it at end of shift, whether it may leave the kiosk — and
//! inventing answers here would bake them into the credential layer where they
//! are hard to change. `verify_pin` returns "this is who it is"; what that is
//! worth is decided one layer up.

use std::sync::OnceLock;

use argon2::password_hash::{
    PasswordHash, PasswordHasher, PasswordVerifier, SaltString, rand_core,
};
use argon2::{Argon2, password_hash::Error as HashError};
use chrono::Utc;
use entity::{org_frontline_members, user_credentials, users};
use oxy_platform::db::establish_connection;
use oxy_shared::errors::OxyError;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, ConnectionTrait, DatabaseBackend,
    DatabaseConnection, EntityTrait, QueryFilter, Set, Statement, TransactionTrait,
};
use uuid::Uuid;

/// `user_credentials.kind` for a PIN. A string rather than an enum so adding a
/// kind is a migration, not a coordinated deploy across every reader.
pub const KIND_PIN: &str = "pin";

/// The rules a PIN is held to. A struct rather than constants so a test can
/// exercise a lockout without waiting fifteen real minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinPolicy {
    pub min_digits: usize,
    pub max_digits: usize,
    /// Consecutive failures before the credential locks.
    pub max_attempts: i32,
    pub lockout_minutes: i64,
}

impl Default for PinPolicy {
    fn default() -> Self {
        Self {
            // Four is what a person will actually use on a wall-mounted tablet
            // between covers, and the scoping + throttle above are what make it
            // safe. Eight is the ceiling because a longer "PIN" is a password
            // and belongs in a different credential kind.
            min_digits: 4,
            max_digits: 8,
            max_attempts: 5,
            lockout_minutes: 15,
        }
    }
}

impl PinPolicy {
    /// Is this string shaped like a PIN this policy would accept?
    ///
    /// Digits only. Letters would widen the space, but they also turn the kiosk
    /// keypad into a keyboard, and a worker who has to find a keyboard writes
    /// the PIN on the wall instead.
    pub fn accepts(&self, pin: &str) -> bool {
        pin.len() >= self.min_digits
            && pin.len() <= self.max_digits
            && pin.bytes().all(|b| b.is_ascii_digit())
    }
}

/// The outcome of [`verify_pin`].
///
/// **Every failure must look the same to the caller.** `WrongPin`, `LockedOut`
/// and `NoSuchWorker` exist so the server can log and meter them differently;
/// [`Self::public_message`] is what may cross the wire, and it is one string.
/// Telling a kiosk "that worker is locked out" confirms the worker exists, and
/// a roster is exactly what an attacker wants enumerated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PinVerdict {
    /// Verified. This is the `users.id` to attribute work to.
    Ok { user_id: Uuid },
    /// The identifier resolved, the PIN did not.
    WrongPin { attempts_remaining: i32 },
    /// The credential is locked and the secret was not even checked.
    LockedOut,
    /// No such credential in this org — or the worker is suspended, or their
    /// PIN was removed. All one variant because the caller must not act on the
    /// difference.
    NoSuchWorker,
    /// The submitted string is not PIN-shaped. Rejected before any database
    /// work, so a malformed request cannot be used to time the lookup.
    Malformed,
    /// The PIN was right, and the caller's admission rule refused this worker
    /// at this door ([`verify_pin_admitting`]). Charged exactly as a wrong PIN
    /// is, so the caller learns the reason and the person at the screen does not.
    NotAdmitted,
}

impl PinVerdict {
    pub fn is_ok(&self) -> bool {
        matches!(self, PinVerdict::Ok { .. })
    }

    /// The single string every failure returns. See the type's docs.
    pub fn public_message(&self) -> &'static str {
        match self {
            PinVerdict::Ok { .. } => "ok",
            _ => "that PIN did not match",
        }
    }
}

/// Hash a PIN for storage. Argon2id with a fresh random salt.
///
/// Not SHA: a 4-digit PIN is 10,000 candidates, so a fast hash means a leaked
/// `user_credentials` dump is every PIN in it within seconds. The cost
/// parameters are argon2's defaults, which are chosen to be slow enough to
/// matter and are the wrong thing to tune without measuring on the serving
/// hardware.
pub fn hash_pin(pin: &str) -> Result<String, OxyError> {
    let salt = SaltString::generate(&mut rand_core::OsRng);
    Argon2::default()
        .hash_password(pin.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| OxyError::RuntimeError(format!("pin hash: {e}")))
}

/// Check a PIN against a stored hash.
///
/// `Ok(false)` is a genuine mismatch; a malformed *stored* hash is an error,
/// not a mismatch, because silently treating an unreadable hash as "wrong PIN"
/// would lock out a whole org after a bad migration and look like user error.
fn pin_matches(pin: &str, stored: &str) -> Result<bool, OxyError> {
    let parsed = PasswordHash::new(stored)
        .map_err(|e| OxyError::RuntimeError(format!("stored pin hash is unreadable: {e}")))?;
    match Argon2::default().verify_password(pin.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(HashError::Password) => Ok(false),
        Err(e) => Err(OxyError::RuntimeError(format!("pin verify: {e}"))),
    }
}

/// Burn roughly the same time a real verify would, when there is nothing to
/// verify against.
///
/// Without this, "no such worker" returns in a fraction of the time a wrong PIN
/// takes, and the difference is measurable over a few hundred requests — which
/// turns the login endpoint into a roster oracle no matter how careful
/// [`PinVerdict::public_message`] is. Hashing a throwaway value is the cheapest
/// way to make the two paths cost the same.
/// `pub` so the *caller* can pay the same cost when it refuses before reaching
/// `verify_pin` at all. An unknown org slug is refused one indexed SELECT in,
/// while a known one always pays an Argon2 verify — so closing the timing
/// channel inside this module is not enough if the layer above can return early.
pub fn burn_verify_time(pin: &str) {
    // Derived from `Argon2::default()` at first use, NOT a hardcoded PHC
    // string. A literal would encode today's parameters (`m=19456,t=2,p=1`),
    // and an argon2 minor bump that changes those defaults would leave the
    // decoy cheaper than a live verify — silently reopening the timing channel
    // this function exists to close, with nothing failing to say so.
    //
    // Hashed once per process: the cost that matters is the VERIFY below,
    // which runs on every call and is the same work the real path does.
    // `OnceLock<Option<..>>` rather than caching the failure as an empty string:
    // a `get_or_init` that stored `String::new()` would pin that value for the
    // life of the process, so one RNG hiccup at startup left the timing channel
    // open forever instead of for one request. Storing `None` lets the next call
    // try again.
    //
    // Unreachable short of `OsRng` failing, which is why it is cheap to be
    // correct about.
    static DECOY: OnceLock<Option<String>> = OnceLock::new();
    let cached = DECOY.get_or_init(|| hash_pin("0000").ok());
    let decoy = match cached {
        Some(d) => d.clone(),
        // Not cached — recompute, and if that fails too there is nothing to
        // verify against, so this degrades to the old fast return.
        None => match hash_pin("0000") {
            Ok(d) => d,
            Err(_) => return,
        },
    };
    if let Ok(parsed) = PasswordHash::new(decoy.as_str()) {
        let _ = Argon2::default().verify_password(pin.as_bytes(), &parsed);
    }
}

/// What one charged failure left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Charged {
    /// The count **after** this attempt, read back from the row the database
    /// actually wrote — not one the caller computed.
    failed_attempts: i32,
    locked: bool,
}

/// Charge one failed attempt, and lock the credential if that was the last one.
///
/// **One statement, and it has to be.** The obvious version — SELECT the row,
/// add one in Rust, write the absolute value back — is a lost update: N
/// requests overlapping between the read and the write all see
/// `failed_attempts = k` and all write `k + 1`, so N guesses cost one attempt.
/// The endpoint a kiosk calls is unauthenticated by construction, so firing
/// concurrently instead of serially is free, and against 10,000 candidates a
/// ceiling of five that only counts one guess per round is not a ceiling.
///
/// Referencing `failed_attempts` and `locked_until` directly in the `SET`
/// expressions is what makes this safe: under READ COMMITTED, Postgres
/// re-evaluates them against the row it just took the lock on, so two
/// concurrent charges land as `k + 1` and `k + 2`. A CTE that pre-read the
/// count would NOT do this — both statements would snapshot `k` — so the
/// repetition below is load-bearing rather than clumsy.
///
/// **A lapsed lockout resets the budget.** Carrying `failed_attempts` across an
/// expired window means the counter is already at the ceiling, so the next
/// single mistyped digit re-locks for a whole window — permanently, one typo
/// away, on a tablet somebody is using mid-shift. Not decaying the count
/// *during* a window is what keeps a slow guesser honest; keeping it *after*
/// one only punishes the worker.
async fn charge_failed_attempt(
    db: &DatabaseConnection,
    credential_id: Uuid,
    policy: PinPolicy,
) -> Result<Charged, OxyError> {
    // `locked_until <= now()` is NULL-safe: a NULL comparison yields NULL, so a
    // credential that has never locked falls through to the ELSE arm.
    const SQL: &str = r#"
        UPDATE user_credentials
           SET failed_attempts = CASE
                   WHEN locked_until <= now() THEN 1
                   ELSE failed_attempts + 1
               END,
               locked_until = CASE
                   WHEN locked_until <= now()
                       THEN CASE WHEN 1 >= $2
                                 THEN now() + ($3::int * interval '1 minute')
                                 ELSE NULL END
                   WHEN failed_attempts + 1 >= $2
                       THEN now() + ($3::int * interval '1 minute')
                   ELSE locked_until
               END
         WHERE id = $1
        RETURNING failed_attempts, locked_until
    "#;
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DatabaseBackend::Postgres,
            SQL,
            [
                credential_id.into(),
                policy.max_attempts.into(),
                (policy.lockout_minutes as i32).into(),
            ],
        ))
        .await
        .map_err(|e| OxyError::DBError(format!("pin attempt: {e}")))?
        .ok_or_else(|| {
            OxyError::DBError("pin attempt: credential vanished mid-verify".to_string())
        })?;

    let failed_attempts: i32 = row
        .try_get("", "failed_attempts")
        .map_err(|e| OxyError::DBError(format!("pin attempt: {e}")))?;
    let locked_until: Option<chrono::DateTime<chrono::FixedOffset>> = row
        .try_get("", "locked_until")
        .map_err(|e| OxyError::DBError(format!("pin attempt: {e}")))?;

    Ok(Charged {
        failed_attempts,
        // Read back rather than recomputed, so the verdict cannot disagree with
        // the row: whichever concurrent charge crossed the ceiling is the one
        // that set the timer, and every later one sees it set.
        locked: locked_until.is_some(),
    })
}

/// Why a charged attempt was refused — for the log, never for the wire.
///
/// Suspension first: charged, then answered as if the worker does not exist. A
/// suspended worker is a former employee, and confirming that their PIN is
/// still the right one is exactly the wrong thing to tell whoever is holding
/// the tablet. A right PIN that reached here was refused by the caller's
/// admission rule; a wrong one stays a wrong one whoever was asking.
fn refused_verdict(charged: Charged, policy: PinPolicy, active: bool, matched: bool) -> PinVerdict {
    if !active {
        PinVerdict::NoSuchWorker
    } else if matched {
        PinVerdict::NotAdmitted
    } else if charged.locked {
        PinVerdict::LockedOut
    } else {
        PinVerdict::WrongPin {
            attempts_remaining: policy.max_attempts - charged.failed_attempts,
        }
    }
}

/// Verify a worker's PIN within one org.
///
/// `identifier` is the login name the kiosk showed in its name picker — stable
/// and org-scoped, not the display name, so renaming a worker does not change
/// how they sign in.
pub async fn verify_pin(
    db: &DatabaseConnection,
    org_id: Uuid,
    identifier: &str,
    pin: &str,
    policy: PinPolicy,
) -> Result<PinVerdict, OxyError> {
    verify_pin_admitting(db, org_id, identifier, pin, policy, |_| true).await
}

/// [`verify_pin`], with the caller's say over WHO may sign in at this door.
///
/// `admits` is asked about the worker the identifier names, and a `false`
/// makes a right PIN count exactly as a wrong one does: the attempt is charged
/// against the same lockout budget, the credential is not stamped as used, and
/// the verdict is [`PinVerdict::NotAdmitted`].
///
/// It has to be decided in here rather than above. By the time `verify_pin`
/// returns `Ok` the success path has already reset `failed_attempts` to zero,
/// so a refusal layered on top would be the one failure that REFUNDS a
/// guesser's budget — and a lockout that arrives later than it should is a
/// channel telling that refusal apart from a wrong PIN.
///
/// Asked for every credential that is found and not locked, whether or not the
/// PIN turns out to be right, so a wrong PIN and a refused right one do the
/// same work in the same order. Synchronous on purpose: whatever the rule needs
/// to read is read by the caller before the PIN is, on every attempt alike.
pub async fn verify_pin_admitting(
    db: &DatabaseConnection,
    org_id: Uuid,
    identifier: &str,
    pin: &str,
    policy: PinPolicy,
    admits: impl FnOnce(Uuid) -> bool,
) -> Result<PinVerdict, OxyError> {
    if !policy.accepts(pin) {
        return Ok(PinVerdict::Malformed);
    }

    let found = user_credentials::Entity::find()
        .filter(user_credentials::Column::Kind.eq(KIND_PIN))
        .filter(user_credentials::Column::OrgId.eq(Some(org_id)))
        .filter(user_credentials::Column::Identifier.eq(identifier))
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(format!("pin lookup: {e}")))?;

    let Some(cred) = found else {
        burn_verify_time(pin);
        return Ok(PinVerdict::NoSuchWorker);
    };

    // Locked: refuse without touching the secret, and WITHOUT extending the
    // lockout. Re-arming the timer on every attempt would let an attacker keep
    // a worker locked out indefinitely just by holding down a button — a
    // denial-of-service handed out for free.
    let now = Utc::now();
    if let Some(until) = cred.locked_until
        && until > now
    {
        // Burn the same time a real verify would, for the same reason the
        // not-found path does. Returning early here skips the Argon2 work, and
        // that is a five-guess roster oracle: lock an identifier out on
        // purpose, then time the sixth response — fast means the worker exists.
        // `public_message` being one string cannot help, because the channel is
        // the clock.
        burn_verify_time(pin);
        return Ok(PinVerdict::LockedOut);
    }

    // Suspension is checked here rather than folded into the query above so a
    // suspended worker's failures still count. Otherwise the lookup misses,
    // `NoSuchWorker` comes back, and the credential never locks — an attacker
    // gets unlimited guesses against exactly the accounts nobody is watching.
    let standing = org_frontline_members::Entity::find_by_id((org_id, cred.user_id))
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(format!("frontline standing: {e}")))?;
    let active = standing
        .as_ref()
        .is_some_and(|s| s.status == org_frontline_members::STATUS_ACTIVE);
    let admitted = admits(cred.user_id);

    let Some(stored) = cred.secret_hash.as_deref() else {
        // A `pin` row with no secret is rejected at the schema level, so this
        // is unreachable rather than merely unlikely. Treat it as "no worker"
        // instead of panicking: an unverifiable credential must never pass.
        tracing::error!(
            credential_id = %cred.id,
            "pin credential has no secret_hash — schema check should have prevented this"
        );
        return Ok(PinVerdict::NoSuchWorker);
    };

    let matched = pin_matches(pin, stored)?;
    if !matched || !active || !admitted {
        let charged = charge_failed_attempt(db, cred.id, policy).await?;
        return Ok(refused_verdict(charged, policy, active, matched));
    }

    let mut update: user_credentials::ActiveModel = cred.clone().into();
    update.failed_attempts = Set(0);
    update.locked_until = Set(None);
    update.last_used_at = Set(Some(now.into()));
    update
        .update(db)
        .await
        .map_err(|e| OxyError::DBError(format!("pin success: {e}")))?;

    Ok(PinVerdict::Ok {
        user_id: cred.user_id,
    })
}

/// Tell a taken identifier apart from a real database failure.
///
/// A duplicate identifier is the ADMIN's mistake, not ours. Re-enrolling
/// somebody who already exists, or reusing a badge number, is the single most
/// likely way enrolment fails and is entirely correctable by the person making
/// it. Left as a bare `DBError` the HTTP layer cannot tell it apart from a pool
/// exhaustion, so it answered 500 "could not enrol the worker" — un-actionable
/// for a normal mistake, and a platform-fault shape in alerting.
///
/// Detected by CONSTRAINT NAME, not by SQLSTATE alone: 23505 only says "some
/// unique index rejected this", while `user_credentials_scoped` is the one that
/// means this identifier is taken in this org. Naming it keeps the message true
/// if another unique index is ever added to the table.
///
/// A free function rather than an inline closure so it can be tested without a
/// database. That matters more than it looks: this is a `contains` over an
/// error `Display` chain spanning sea-orm and sqlx, and if either changes its
/// formatting the mapping silently reverts to the 500 it exists to remove —
/// with nothing failing to say so.
fn classify_credential_error(e: &sea_orm::DbErr) -> OxyError {
    if e.to_string().contains("user_credentials_scoped") {
        OxyError::ValidationError(IDENTIFIER_TAKEN.to_string())
    } else {
        OxyError::DBError(format!("enrol credential: {e}"))
    }
}

/// The message an unknown worker produces, as a `ValidationError`.
///
/// Named for the same reason as [`IDENTIFIER_TAKEN`]: the HTTP layer answers
/// **404** for this one specifically. Blanket-mapping the whole variant worked
/// while this function had exactly one validation, and would have quietly
/// reported the NEXT one — a bad status, a self-suspend guard — as "worker not
/// found".
pub const WORKER_NOT_FOUND: &str = "no frontline worker with that id in this org";

/// The message a duplicate identifier produces, as a `ValidationError`.
///
/// A constant rather than a literal because the HTTP layer matches on it to
/// answer **409** rather than 400 — a taken badge number is a conflict, not a
/// malformed request. `OxyError` has no `Conflict` variant and adding one to a
/// shared enum for this single case is a wider change than it earns, so the two
/// sides agree on this name instead of on a string typed twice.
pub const IDENTIFIER_TAKEN: &str = "a worker with that identifier is already enrolled in this org";

/// Enrol a worker who has no email address.
///
/// Three rows in one transaction — the `users` row with a NULL email, the
/// `org_frontline_members` binding, and the PIN credential. All three or none:
/// a user with standing but no credential cannot sign in, and a credential
/// without standing is a login that grants nothing, so a partial enrolment is
/// a support ticket either way.
///
/// Returns the new `users.id`.
pub async fn enroll_worker(
    db: &DatabaseConnection,
    org_id: Uuid,
    display_name: &str,
    identifier: &str,
    pin: &str,
    policy: PinPolicy,
) -> Result<Uuid, OxyError> {
    if !policy.accepts(pin) {
        return Err(OxyError::ValidationError(format!(
            "a PIN must be {}–{} digits",
            policy.min_digits, policy.max_digits
        )));
    }
    if display_name.trim().is_empty() || identifier.trim().is_empty() {
        return Err(OxyError::ValidationError(
            "display name and identifier are both required".to_string(),
        ));
    }
    // Hash BEFORE opening the transaction. Argon2 is deliberately slow, and
    // holding a write transaction open across it would serialise enrolment
    // behind the one thing in this function designed to take time.
    let hash = hash_pin(pin)?;

    let txn = db
        .begin()
        .await
        .map_err(|e| OxyError::DBError(format!("enrol begin: {e}")))?;
    let user_id = Uuid::new_v4();

    users::ActiveModel {
        id: Set(user_id),
        // The whole point. A NULL here is what keeps this person out of every
        // email-keyed path — OAuth collapse, Slack matching, invitations,
        // platform grants — by construction rather than by a check.
        email: Set(None),
        name: Set(display_name.trim().to_string()),
        picture: Set(None),
        email_verified: Set(false),
        magic_link_token: ActiveValue::NotSet,
        magic_link_token_expires_at: ActiveValue::NotSet,
        status: Set(users::UserStatus::Active),
        created_at: ActiveValue::NotSet,
        last_login_at: ActiveValue::NotSet,
    }
    .insert(&txn)
    .await
    .map_err(|e| OxyError::DBError(format!("enrol user: {e}")))?;

    org_frontline_members::ActiveModel {
        org_id: Set(org_id),
        user_id: Set(user_id),
        status: Set(org_frontline_members::STATUS_ACTIVE.to_string()),
        created_at: ActiveValue::NotSet,
    }
    .insert(&txn)
    .await
    .map_err(|e| OxyError::DBError(format!("enrol standing: {e}")))?;

    user_credentials::ActiveModel {
        id: Set(Uuid::new_v4()),
        user_id: Set(user_id),
        kind: Set(KIND_PIN.to_string()),
        org_id: Set(Some(org_id)),
        identifier: Set(identifier.trim().to_string()),
        secret_hash: Set(Some(hash)),
        failed_attempts: Set(0),
        locked_until: Set(None),
        created_at: ActiveValue::NotSet,
        last_used_at: Set(None),
    }
    .insert(&txn)
    .await
    .map_err(|e| classify_credential_error(&e))?;

    txn.commit()
        .await
        .map_err(|e| OxyError::DBError(format!("enrol commit: {e}")))?;
    tracing::info!(%org_id, %user_id, "enrolled a frontline worker");
    Ok(user_id)
}

/// Convenience wrapper for callers holding no connection of their own.
pub async fn verify_pin_with_default_db(
    org_id: Uuid,
    identifier: &str,
    pin: &str,
) -> Result<PinVerdict, OxyError> {
    let db = establish_connection().await?;
    verify_pin(&db, org_id, identifier, pin, PinPolicy::default()).await
}

/// Is this user a frontline worker in this org, and active?
///
/// The fact `oxy-authz` will read as `frontline_orgs`. Separate from
/// [`verify_pin`] because a request arrives with a session, not a PIN.
pub async fn is_active_frontline(
    db: &impl ConnectionTrait,
    org_id: Uuid,
    user_id: Uuid,
) -> Result<bool, OxyError> {
    Ok(org_frontline_members::Entity::find_by_id((org_id, user_id))
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(format!("frontline standing: {e}")))?
        .is_some_and(|s| s.status == org_frontline_members::STATUS_ACTIVE))
}

#[cfg(test)]
mod tests {

    /// The 409 path depends on a `contains` over an error Display chain that
    /// spans sea-orm and sqlx. If either reformats, the mapping reverts to the
    /// 500 it was written to remove — and nothing else in the suite notices.
    ///
    /// The real message shape, copied from what Postgres emits through that
    /// chain, rather than a string invented to satisfy the assertion.
    #[test]
    fn a_taken_identifier_is_told_apart_from_a_real_database_failure() {
        let dup = sea_orm::DbErr::Query(sea_orm::RuntimeErr::Internal(
            "error returned from database: duplicate key value violates unique \
             constraint \"user_credentials_scoped\""
                .to_string(),
        ));
        match classify_credential_error(&dup) {
            OxyError::ValidationError(m) => assert_eq!(m, IDENTIFIER_TAKEN),
            other => panic!("a duplicate identifier must be admin-correctable, got {other:?}"),
        }

        // The OTHER unique index on this table. It cannot fire for a PIN
        // credential (org_id is always Some, and that index is WHERE org_id IS
        // NULL), so it must NOT be reported as a taken badge — that would tell
        // an admin to change an identifier over an email collision.
        let global = sea_orm::DbErr::Query(sea_orm::RuntimeErr::Internal(
            "duplicate key value violates unique constraint \"user_credentials_global\""
                .to_string(),
        ));
        assert!(
            matches!(classify_credential_error(&global), OxyError::DBError(_)),
            "only the scoped index means the badge is taken"
        );

        // Infrastructure stays infrastructure. This is the case the whole
        // distinction exists for: it must never read as the admin's mistake.
        let pool = sea_orm::DbErr::Conn(sea_orm::RuntimeErr::Internal(
            "pool timed out while waiting for an open connection".to_string(),
        ));
        assert!(
            matches!(classify_credential_error(&pool), OxyError::DBError(_)),
            "a pool exhaustion is ours, not the admin's"
        );
    }

    use super::*;

    #[test]
    fn a_pin_round_trips_and_a_near_miss_does_not() {
        let hash = hash_pin("4821").expect("hash");
        assert!(pin_matches("4821", &hash).expect("verify"));
        assert!(!pin_matches("4822", &hash).expect("verify"));
        assert!(!pin_matches("482", &hash).expect("verify"));
    }

    #[test]
    fn the_same_pin_hashes_differently_every_time() {
        // A per-credential random salt. Without it, two workers who both chose
        // 1234 are visibly the same row to anyone reading the table, which
        // turns one cracked PIN into every account that shares it.
        let a = hash_pin("1234").expect("hash");
        let b = hash_pin("1234").expect("hash");
        assert_ne!(a, b);
        assert!(pin_matches("1234", &a).expect("verify"));
        assert!(pin_matches("1234", &b).expect("verify"));
    }

    #[test]
    fn an_unreadable_stored_hash_errors_rather_than_reading_as_a_wrong_pin() {
        // The difference matters operationally: a mismatch charges an attempt
        // and eventually locks the worker out, so silently treating a corrupt
        // hash as "wrong PIN" would lock an entire org after a bad backfill and
        // look exactly like everyone forgetting their PIN at once.
        assert!(pin_matches("1234", "not-a-phc-string").is_err());
    }

    #[test]
    fn policy_rejects_what_is_not_pin_shaped() {
        let p = PinPolicy::default();
        assert!(p.accepts("4821"));
        assert!(p.accepts("48211234"));
        assert!(!p.accepts("482"), "too short");
        assert!(!p.accepts("482112345"), "too long");
        assert!(
            !p.accepts("48a1"),
            "letters turn the keypad into a keyboard"
        );
        assert!(!p.accepts(""), "empty");
        assert!(!p.accepts("48 1"), "whitespace");
    }

    #[test]
    fn every_failure_says_the_same_thing() {
        // The property that keeps the login endpoint from being a roster
        // oracle. If a new variant is added without a public_message arm, this
        // fails — which is the point.
        let failures = [
            PinVerdict::WrongPin {
                attempts_remaining: 3,
            },
            PinVerdict::LockedOut,
            PinVerdict::NoSuchWorker,
            PinVerdict::Malformed,
            PinVerdict::NotAdmitted,
        ];
        let messages: Vec<_> = failures.iter().map(|v| v.public_message()).collect();
        assert!(
            messages.windows(2).all(|w| w[0] == w[1]),
            "a caller must not be able to tell failures apart: {messages:?}"
        );
        assert!(failures.iter().all(|v| !v.is_ok()));
    }

    /// A charged refusal is named for the log in a fixed order: suspension
    /// hides everything, a right PIN that reached the charge was the caller's
    /// admission refusing it, and a wrong PIN stays a wrong PIN whoever asked.
    #[test]
    fn a_charged_refusal_names_its_reason_in_order() {
        let p = PinPolicy::default();
        let open = Charged {
            failed_attempts: 2,
            locked: false,
        };
        let locked = Charged {
            failed_attempts: p.max_attempts,
            locked: true,
        };
        assert_eq!(
            refused_verdict(open, p, false, true),
            PinVerdict::NoSuchWorker
        );
        assert_eq!(
            refused_verdict(open, p, true, true),
            PinVerdict::NotAdmitted
        );
        assert_eq!(
            refused_verdict(locked, p, true, true),
            PinVerdict::NotAdmitted,
            "a right PIN refused at this door is logged as that, even as it locks"
        );
        assert_eq!(
            refused_verdict(open, p, true, false),
            PinVerdict::WrongPin {
                attempts_remaining: p.max_attempts - 2
            }
        );
        assert_eq!(
            refused_verdict(locked, p, true, false),
            PinVerdict::LockedOut
        );
    }

    #[test]
    fn the_decoy_costs_what_a_real_verify_costs() {
        // `burn_verify_time` is only worth its round-trip if it does the same
        // work the real path does. Pinning the PARAMETERS rather than a literal
        // hash is the point: the decoy is derived from `Argon2::default()`, so
        // an argon2 bump that changes the defaults moves both together instead
        // of leaving the decoy cheap.
        let decoy = hash_pin("0000").expect("decoy");
        let real = hash_pin("4821").expect("real");
        let dp = PasswordHash::new(&decoy).expect("decoy parses");
        let rp = PasswordHash::new(&real).expect("real parses");
        assert_eq!(dp.algorithm, rp.algorithm);
        assert_eq!(
            dp.params, rp.params,
            "the decoy must carry the same cost parameters as a stored PIN"
        );
        // And it must not match anything a caller could send.
        assert!(!pin_matches("4821", &decoy).expect("verify"));
    }
}

/// Suspend a frontline worker, or reinstate one.
///
/// # Suspend, never delete
///
/// A worker who leaves keeps their row. Their submissions, findings and
/// completed training all point at `users.id`, and deleting the person would
/// either orphan that history or cascade it away — so the record of who did the
/// work would disappear along with the ability to sign in, which is the wrong
/// half to lose. `org_frontline_members.status` is the switch, and it was
/// modelled from the start; this is the writer it never had.
///
/// # What suspension already does, without any further change
///
/// `verify_pin` reads the same status and answers `NoSuchWorker` on a suspended
/// worker — after charging the failed attempt, so the account still locks
/// rather than offering unlimited guesses. It deliberately does NOT say "your
/// account is suspended": confirming that a former employee's PIN is still
/// correct is exactly the wrong thing to tell whoever is holding the tablet.
/// The roster route filters on the same column, so a suspended worker leaves
/// the name picker too.
///
/// So this function changes one column and three behaviours follow. The
/// credential is left in place on purpose — reinstating is then a second call
/// here rather than a re-enrolment that would mint a new PIN and make the
/// worker learn a new one.
///
/// Returns whether the row MOVED. A no-op is reported as `false` rather than as
/// an error: setting a suspended worker suspended is the caller getting what
/// they asked for, and a 409 there would make an idempotent retry look like a
/// conflict.
/// Replace a worker's PIN and clear any lockout the old one accumulated.
///
/// The one PIN operation a manager needs after enrolment: a forgotten PIN is
/// re-issued at the counter, and the lockout that forgetting it produced must
/// not outlive the reset — a worker who has just been handed a new PIN and is
/// still locked out for ten minutes is a worker who thinks the new PIN is
/// wrong too. Same policy as enrolment; the new PIN is never stored, logged or
/// returned.
pub async fn reset_pin(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    pin: &str,
    policy: PinPolicy,
) -> Result<(), OxyError> {
    if !policy.accepts(pin) {
        return Err(OxyError::ValidationError(format!(
            "a PIN must be {}–{} digits",
            policy.min_digits, policy.max_digits
        )));
    }
    if org_frontline_members::Entity::find_by_id((org_id, user_id))
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(format!("frontline standing: {e}")))?
        .is_none()
    {
        return Err(OxyError::ValidationError(WORKER_NOT_FOUND.to_string()));
    }
    let hash = hash_pin(pin)?;
    let res = user_credentials::Entity::update_many()
        .col_expr(
            user_credentials::Column::SecretHash,
            sea_orm::sea_query::Expr::value(Some(hash)),
        )
        .col_expr(
            user_credentials::Column::FailedAttempts,
            sea_orm::sea_query::Expr::value(0i32),
        )
        .col_expr(
            user_credentials::Column::LockedUntil,
            sea_orm::sea_query::Expr::value(Option::<chrono::DateTime<chrono::FixedOffset>>::None),
        )
        .filter(user_credentials::Column::UserId.eq(user_id))
        .filter(user_credentials::Column::OrgId.eq(Some(org_id)))
        .filter(user_credentials::Column::Kind.eq(KIND_PIN))
        .exec(db)
        .await
        .map_err(|e| OxyError::DBError(format!("reset pin: {e}")))?;
    if res.rows_affected == 0 {
        return Err(OxyError::ValidationError(WORKER_NOT_FOUND.to_string()));
    }
    tracing::info!(%org_id, %user_id, "frontline PIN reset");
    Ok(())
}

pub async fn set_worker_standing(
    db: &DatabaseConnection,
    org_id: Uuid,
    user_id: Uuid,
    active: bool,
) -> Result<bool, OxyError> {
    let want = if active {
        org_frontline_members::STATUS_ACTIVE
    } else {
        org_frontline_members::STATUS_SUSPENDED
    };

    let Some(row) = org_frontline_members::Entity::find_by_id((org_id, user_id))
        .one(db)
        .await
        .map_err(|e| OxyError::DBError(format!("frontline standing: {e}")))?
    else {
        // Scoped to the org in the primary key, so this is also what a caller
        // gets for somebody else's worker — an admin cannot discover that a
        // user id belongs to another tenant's roster.
        return Err(OxyError::ValidationError(WORKER_NOT_FOUND.to_string()));
    };

    // The WRITE decides whether anything moved, not the read above it.
    //
    // The read stays, because it is what tells an unknown worker from a no-op —
    // one is a 404 and the other is a 200. But deriving `changed` from it made
    // the field a read-then-write: two concurrent suspends both saw `active` and
    // both answered `changed: true`, while only one of them moved a row. The
    // final state was right either way; the field was not, and its whole purpose
    // is to be believed by a caller reconciling a roster.
    let moved = org_frontline_members::Entity::update_many()
        .col_expr(
            org_frontline_members::Column::Status,
            sea_orm::sea_query::Expr::value(want),
        )
        .filter(org_frontline_members::Column::OrgId.eq(org_id))
        .filter(org_frontline_members::Column::UserId.eq(user_id))
        // The guard that makes this the arbiter: a row already in the target
        // state matches nothing, so `rows_affected` is the answer rather than a
        // restatement of what we read a moment ago.
        .filter(org_frontline_members::Column::Status.ne(want))
        .exec(db)
        .await
        .map_err(|e| OxyError::DBError(format!("set frontline standing: {e}")))?
        .rows_affected
        == 1;

    // Reinstating clears the lockout that SUSPENSION caused.
    //
    // `verify_pin` charges a failed attempt against a suspended worker before
    // answering `NoSuchWorker` — deliberately, so the credential still locks
    // rather than offering unlimited guesses at exactly the accounts nobody is
    // watching. The consequence is that attempts accrue on a credential nobody
    // is supposed to be using, and the lockout check runs BEFORE the standing
    // check: so a worker whose PIN was tried while they were gone comes back to
    // `LockedOut` on the correct PIN.
    //
    // It self-heals after `lockout_minutes`, which is why this is a correctness
    // nicety rather than a hole. But reinstatement is the moment somebody has
    // decided this person may work again, and this PR's own promise is that it
    // restores the original PIN — a fifteen-minute wait nobody can explain is
    // not that.
    //
    // Scoped to the PIN credential in this org: an email credential has no
    // lockout of this kind and is not this function's business.
    if active && moved {
        user_credentials::Entity::update_many()
            .col_expr(
                user_credentials::Column::FailedAttempts,
                sea_orm::sea_query::Expr::value(0),
            )
            .col_expr(
                user_credentials::Column::LockedUntil,
                sea_orm::sea_query::Expr::value(
                    Option::<sea_orm::prelude::DateTimeWithTimeZone>::None,
                ),
            )
            .filter(user_credentials::Column::UserId.eq(user_id))
            .filter(user_credentials::Column::Kind.eq(KIND_PIN))
            .filter(user_credentials::Column::OrgId.eq(Some(org_id)))
            .exec(db)
            .await
            .map_err(|e| OxyError::DBError(format!("clear frontline lockout: {e}")))?;
    }

    if moved {
        tracing::info!(%org_id, %user_id, standing = want, "frontline standing changed");
    }
    Ok(moved)
}
