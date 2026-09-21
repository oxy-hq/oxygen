//! Frontline sign-in — the HTTP surface a kiosk talks to.
//!
//! `oxy_auth::frontline` decides whether a PIN is right. This decides what that
//! is worth: a session, scoped and short, that lets a worker be somebody for
//! the length of a shift.
//!
//! # Why these routes are public, and what stands in for auth
//!
//! A worker has nothing to authenticate WITH until they have signed in, so both
//! routes below sit in the public router. That makes rate limiting load-bearing
//! rather than defensive, and it is layered:
//!
//! * the credential itself throttles and locks out per `(org, identifier)`
//!   ([`oxy_auth::frontline::verify_pin`]);
//! * this module throttles per **org** as well, because the credential-level
//!   lockout is per worker and a caller walking a roster of 40 names gets 40
//!   separate budgets;
//! * every failure returns one response, so neither layer leaks which of the
//!   two refused — including a right PIN from someone not rostered at the
//!   kiosk's store, which is charged and answered as a wrong one. A store with
//!   NOBODY rostered answers every attempt the same way and charges no one,
//!   because that refusal is the kiosk's and says nothing about the person.
//!
//! # Fleet role
//!
//! Both routes are `route_fleet`, and must be. They read and write Postgres and
//! touch no working copy, no `.git` and no state dir — and more to the point,
//! **signing in has to survive the ide restarting**. Pinning login to the
//! singleton would mean a deploy locks every store out of its own checklists.

use crate::server::api::middlewares::role_guards::OrgAdmin;
use crate::server::api::operating_graph::assignments;
use crate::server::api::operating_graph::dto::AssignmentSpec;
use axum::extract::{Path, Query};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::{Json, http::header};
use entity::{org_frontline_members, org_role_members, organizations, user_credentials, users};
use oxy::database::client::establish_connection;
use oxy_app_core::audit;
use oxy_auth::extractor::AuthenticatedUserExtractor;
use oxy_auth::frontline::{self, KIND_PIN, PinPolicy, PinVerdict};
use oxy_shared::errors::OxyError;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use tracing::{error, info, instrument, warn};
use uuid::Uuid;

/// How long a shift session lasts.
///
/// Twelve hours, not the week a magic-link session gets: this credential was
/// proved by four digits typed on a shared tablet in a room full of people, and
/// the tablet does not leave the store. A closing shift is the long case.
///
/// It is a CEILING, not the working rule. What actually ends an unattended
/// shift is the kiosk's idle timeout
/// (`frontline_devices::DEFAULT_IDLE_TIMEOUT_SECONDS`), which is why that
/// module reads this constant rather than keeping its own copy of twelve.
pub(crate) const SHIFT_HOURS: i64 = 12;

/// Per-org attempt ceiling within [`ORG_WINDOW`].
///
/// The credential's own lockout is per worker, so a caller trying `0000` once
/// against each of 40 names never trips it — 40 identifiers, one attempt each.
/// This is the ceiling that notices the *pattern* rather than the account.
const ORG_ATTEMPT_CEILING: usize = 30;
const ORG_WINDOW: Duration = Duration::from_secs(60);

/// Failed attempts per org, newest last. In-process on purpose: this is a
/// coarse brake in front of the real per-credential throttle, not the throttle
/// itself, so a per-replica window is the right cost. Making it shared state
/// would put a Postgres write on every wrong keypress to slow down an attacker
/// the credential layer already locks out.
static ORG_ATTEMPTS: LazyLock<Mutex<HashMap<Uuid, Vec<Instant>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn org_is_rate_limited(org_id: Uuid) -> bool {
    // A poisoned lock means a previous holder panicked. RECOVER rather than
    // fail closed: poisoning is permanent, so `Err(_) => return true` refused
    // every sign-in for every org for the life of the process — a self-inflicted
    // outage far larger than the guessing it was meant to stop. The guarded data
    // is a map of timestamps with no invariant a panic could leave broken, so
    // there is nothing to protect by refusing to read it.
    let mut map = match ORG_ATTEMPTS.lock() {
        Ok(m) => m,
        Err(poisoned) => poisoned.into_inner(),
    };
    let now = Instant::now();
    let hits = map.entry(org_id).or_default();
    hits.retain(|t| now.duration_since(*t) < ORG_WINDOW);
    hits.len() >= ORG_ATTEMPT_CEILING
}

fn record_org_attempt(org_id: Uuid) {
    let mut map = match ORG_ATTEMPTS.lock() {
        Ok(m) => m,
        Err(poisoned) => poisoned.into_inner(),
    };
    map.entry(org_id).or_default().push(Instant::now());
}

/// The throttle key for a request that named an org that does not exist.
///
/// There is no org id to key on, so key on the absence of one: a caller walking
/// slugs is one caller, and this bucket has no legitimate traffic to starve —
/// the worst it costs a real user is a `429` on a typo'd slug.
const NO_SUCH_ORG: Uuid = Uuid::nil();

#[derive(Debug, Deserialize)]
pub struct RosterQuery {
    /// Org slug — the kiosk knows which store it is bolted to.
    pub org: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RosterEntry {
    /// The stable login name the kiosk sends back with the PIN. Not the
    /// display name: renaming a worker must not change how they sign in.
    pub identifier: String,
    pub name: String,
}

/// The name picker.
///
/// # Whose names it shows
///
/// **The tablet's store, not the tenant.** A kiosk bound to a location lists
/// only workers who hold an assignment AT that location
/// (`org_role_members.location_id`, the operating graph's roster — see
/// `operating_graph::assignments`). A kiosk with no location keeps the
/// org-wide list, because there is no store to narrow to.
///
/// Two exclusions are deliberate rather than oversights:
///
/// * **An org-wide position does not appear.** `org_role_members` stores
///   `location_id IS NULL` for a franchisor-scope role, and an Account Manager
///   who works across every store signs in on the web — putting them on every
///   tablet in the chain is the org-wide picker again under another name.
/// * **There is no per-assignment status column.** Standing is the worker's
///   (`org_frontline_members.status`), already filtered below, so "active
///   assignment" means the worker is active and the assignment row exists.
///   Un-rostering somebody from a store is a delete, and it takes them off
///   that store's picker.
///
/// Until 2026-09-11 this listed every PIN holder in the org, so a Santa Rosa
/// manager appeared on the Clovis tablet — a name picker of 127 strangers,
/// and a wider guessing surface for anyone standing at the counter.
///
/// **The store narrows the read; it does not filter its result.** The store's
/// rostered user ids are resolved first and the credential query is filtered by
/// them, so [`ROSTER_LIMIT`] counts this store's people. Narrowing afterwards
/// meant a tenant past 200 PIN credentials lost the names that sort late from
/// the store's own picker — silently: no error, no empty list, the worker
/// simply was not there.
///
/// **Sign-in admits exactly who the picker shows.** `login` asks the same
/// scope the same rule ([`verify_at_kiosk`]), so a worker rostered only at
/// another store is refused on this tablet — as a wrong PIN is. The reach a
/// signed-in worker then holds is still the operating graph's business, not
/// this route's.
///
/// **Deleting a location re-widens its tablets.** `org_kiosk_devices.location_id`
/// is `ON DELETE SET NULL`, and `NULL` here means org-wide, so a kiosk whose
/// place was deleted falls back to the whole tenant's list — and signs in the
/// whole tenant. Re-bind or revoke such a kiosk rather than leaving it.
///
/// # What it still does not carry
///
/// Deliberately **no** credential material and **no** lockout state. It is
/// rendered on a screen anyone in the building can see, so it must not help
/// an attacker choose a target — "this one is locked out" would confirm both
/// that the worker exists and that somebody has been guessing at them.
///
/// It does leak the roster of one store to anyone holding that store's kiosk
/// cookie. That is a deliberate trade and the reason the PIN is not the only
/// control: the tablet is on the wall, the names are on the schedule beside
/// it, and a name picker nobody can load is a kiosk nobody can use.
#[instrument(skip_all, fields(org = %q.org))]
pub async fn roster(headers: HeaderMap, Query(q): Query<RosterQuery>) -> axum::response::Response {
    // The body depends on the kiosk cookie; see `frontline_devices::no_store`.
    let mut resp = roster_body(&headers, q).await;
    super::frontline_devices::no_store(&mut resp);
    resp
}

async fn roster_body(headers: &HeaderMap, q: RosterQuery) -> axum::response::Response {
    let Ok(db) = establish_connection().await else {
        // `{"staff": []}`, not `{}` — a kiosk reads `body.staff` and would get
        // `undefined` from the bare object, which is a render crash rather than
        // an empty picker.
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "staff": [] })),
        )
            .into_response();
    };

    // Only an enrolled kiosk of this org sees its roster. The doc on `login`
    // records that this read disclosed org existence by content; behind the
    // device it discloses nothing to anyone who is not already standing at
    // the counter, and the same empty answer covers "no kiosk", "wrong org"
    // and "no such org".
    let device = super::frontline_devices::bound_device(&db, headers).await;
    let bound_to = device.as_ref().map(|d| d.org_id);
    let Ok(Some(org)) = organizations::Entity::find()
        .filter(organizations::Column::Slug.eq(&q.org))
        .one(&db)
        .await
    else {
        // An unknown org answers as an EMPTY roster, not a 404. A 404 here is
        // an org-slug oracle, and slugs are guessable.
        return Json(serde_json::json!({ "staff": [] })).into_response();
    };

    if bound_to != Some(org.id) {
        return Json(serde_json::json!({ "staff": [] })).into_response();
    }

    // WHERE the tablet is decides WHO the credential read may even return, and
    // it is resolved BEFORE that read so the row cap below counts this store's
    // people rather than the tenant's.
    let location = device.and_then(|d| d.location_id);
    // A failed read shows nobody, as an empty roster does: closed, never the
    // tenant. Closed but not silent — the `warn` is what separates "nobody is
    // rostered here" from "the query failed" for whoever is staring at an empty
    // picker. The door does not fold the two together; see `verify_at_kiosk`.
    let scope = roster_scope(&db, org.id, location)
        .await
        .unwrap_or_else(|e| {
            warn!(error = %e, org_id = %org.id, location = ?location,
                "roster assignment read failed; the picker will be empty");
            RosterScope::Nobody
        });
    if scope == RosterScope::Nobody {
        return Json(serde_json::json!({ "staff": [] })).into_response();
    }

    let rows = credential_query(org.id, &scope)
        .all(&db)
        .await
        .unwrap_or_default();

    let candidates = active_named(&db, org.id, rows).await;

    let candidates_in = candidates.len();
    let staff = on_this_kiosk(candidates, &scope, location);
    info!(org_id = %org.id, location = ?location, candidates = candidates_in, staff = staff.len(),
        "frontline roster narrowed");
    Json(serde_json::json!({ "staff": staff })).into_response()
}

/// How many names one picker may carry.
///
/// A roster is a screen, not a dataset: the cap is what stops a large tenant
/// turning the picker into a slow query on every kiosk load. What it counts is
/// [`RosterScope`]'s business.
const ROSTER_LIMIT: u64 = 200;

/// How many rostered people one store's narrowing may consider.
///
/// Deliberately larger than [`ROSTER_LIMIT`], because this read NARROWS the
/// credential query rather than sizing the picker — the picker's own cap is the
/// `LIMIT` on that query. Capping both at 200 would re-create the bug one level
/// down: a store's full roster includes people who hold no PIN (a manager with
/// an account, an office user), so cutting it at the picker's size could drop
/// PIN holders before the credential read ever saw them. Still bounded, and
/// still one column of uuids for one location — this route is public.
///
/// Past it the scope is TRUNCATED, and not to "the first N" of anything: the
/// read orders by `user_id`, a random v4, so the cut is stable across loads but
/// arbitrary. Sign-in shares this scope ([`verify_at_kiosk`]), so whoever falls
/// past the ceiling is neither on that store's picker nor able to sign in on its
/// tablet — refused as a wrong PIN is. [`roster_scope`] logs a `warn` when a
/// read comes back at the ceiling, so a store that ever reaches it is
/// diagnosable rather than silent.
const STORE_ROSTER_SCAN_LIMIT: u64 = 2_000;

/// What the credential read is narrowed to **before** [`ROSTER_LIMIT`] applies.
///
/// Until this existed the cap was org-wide and the store filter ran on whatever
/// survived it, so a tenant past 200 PIN credentials silently lost the names
/// that sort late from every store whose crew happened to be among them — no
/// error, no empty list, the worker simply was not on their own tablet.
/// Narrowing first makes the cap per store, which is the unit a picker is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RosterScope {
    /// A kiosk enrolled without a place. There is no store to narrow to, so
    /// the cap stays org-wide — today's behaviour for those, kept.
    Org,
    /// The user ids rostered at this tablet's store. Never empty: an empty
    /// narrowing is [`RosterScope::Nobody`], because `IN ()` is not a filter.
    Store(Vec<Uuid>),
    /// Show nobody — nobody is rostered here. An empty picker, never the whole
    /// tenant. A read that FAILED is not this: [`roster_scope`] answers `Err`
    /// for it, because the picker and the door must not pay for a server
    /// failure alike (see [`verify_at_kiosk`]).
    Nobody,
}

/// The credential read, narrowed and capped in that order.
///
/// Built rather than run so the shape can be asserted in a unit test: the
/// property this route lives or dies by is that the `user_id IN (…)` and the
/// `LIMIT` land in the SAME statement.
fn credential_query(
    org_id: Uuid,
    scope: &RosterScope,
) -> sea_orm::Select<user_credentials::Entity> {
    let base = user_credentials::Entity::find()
        .filter(user_credentials::Column::Kind.eq(KIND_PIN))
        .filter(user_credentials::Column::OrgId.eq(Some(org_id)));
    let narrowed = match scope {
        RosterScope::Store(ids) => base.filter(user_credentials::Column::UserId.is_in(ids.clone())),
        // Nobody never reaches a query — the caller answers with an empty
        // picker instead — but the match is exhaustive so adding a scope
        // cannot silently inherit the org-wide read.
        RosterScope::Org | RosterScope::Nobody => base,
    };
    narrowed
        .order_by_asc(user_credentials::Column::Identifier)
        .limit(ROSTER_LIMIT)
}

/// The scope read back as the `(user_id, location_id)` pairs
/// [`narrow_to_location`] takes.
///
/// Redundant by construction — [`credential_query`] already asked the database
/// for exactly these user ids — and kept anyway: `narrow_to_location` is the
/// store rule in one pure, tested place, so an edit that widens the CREDENTIAL
/// query cannot widen a store's picker without failing a test. It does not
/// re-check [`roster_scope`]'s own filter: every pair is stamped with the
/// tablet's location, because that is the only location the scope was read at,
/// so a widened scope read would pass through here unnoticed.
fn scope_assignments(scope: &RosterScope, location: Option<Uuid>) -> Vec<(Uuid, Option<Uuid>)> {
    match scope {
        RosterScope::Store(ids) => ids.iter().map(|id| (*id, location)).collect(),
        RosterScope::Org | RosterScope::Nobody => Vec::new(),
    }
}

/// Who is rostered at the tablet's store, as one bounded read of distinct user
/// ids — and only when the tablet has a place to narrow to, so an org whose
/// kiosks carry no location pays nothing for a rule that cannot apply to it.
///
/// `DISTINCT` on purpose: a worker holding two roles at one store is one name
/// on the picker, and without it [`STORE_ROSTER_SCAN_LIMIT`] would count their
/// rows twice and cut the tail of the store's own roster off again. Ordered so
/// that a store big enough to reach that ceiling truncates to the same set on
/// every load instead of a different one each time — the same set, not a
/// meaningful one (see [`STORE_ROSTER_SCAN_LIMIT`]), which is why reaching it
/// is a `warn`.
///
/// A failed read is `Err`, NOT [`RosterScope::Nobody`], and each caller decides
/// what it costs. Both fail closed — nobody on the picker, nobody through the
/// door, never the whole tenant's crew because a query blipped — but only the
/// picker can treat it as "nobody". At the door `Nobody` is a `401`, true of the
/// store until someone is assigned there; a failed read is ours, and says so
/// with a `503` a tablet backs off from, rather than telling a worker that the
/// right PIN did not match. Neither charges the worker (see [`verify_at_kiosk`]).
async fn roster_scope(
    db: &sea_orm::DatabaseConnection,
    org_id: Uuid,
    location: Option<Uuid>,
) -> Result<RosterScope, sea_orm::DbErr> {
    let Some(at) = location else {
        return Ok(RosterScope::Org);
    };
    let rostered = org_role_members::Entity::find()
        .select_only()
        .column(org_role_members::Column::UserId)
        .distinct()
        .filter(org_role_members::Column::OrgId.eq(org_id))
        .filter(org_role_members::Column::LocationId.eq(at))
        .order_by_asc(org_role_members::Column::UserId)
        .limit(STORE_ROSTER_SCAN_LIMIT)
        .into_tuple::<Uuid>()
        .all(db)
        .await?;
    if rostered.len() as u64 >= STORE_ROSTER_SCAN_LIMIT {
        warn!(org_id = %org_id, location = %at, limit = STORE_ROSTER_SCAN_LIMIT,
            "this store's roster reached the scan ceiling; people past it are neither on \
             its picker nor able to sign in on its tablet");
    }
    if rostered.is_empty() {
        info!(org_id = %org_id, location = %at, "nobody is rostered at this kiosk's store");
        return Ok(RosterScope::Nobody);
    }
    Ok(RosterScope::Store(rostered))
}

/// The credentials that belong to an active worker, paired with the name to
/// show — and in the `identifier` order the query asked for, because a picker
/// whose order changes between loads is a picker people mis-tap.
///
/// Names come from `users`, and only for workers whose standing is active: a
/// suspended worker must not appear on the picker at all.
///
/// Two batched queries, not two per credential. [`ROSTER_LIMIT`] caps the ROW
/// count, not the QUERY count, and this route is public, unthrottled
/// (`org_is_rate_limited` guards `login` only) and answers for any guessable
/// slug — so the per-row shape made a trivial loop a 400x amplifier against the
/// shared pool.
async fn active_named(
    db: &sea_orm::DatabaseConnection,
    org_id: Uuid,
    rows: Vec<user_credentials::Model>,
) -> Vec<(Uuid, RosterEntry)> {
    let ids: Vec<Uuid> = rows.iter().map(|c| c.user_id).collect();
    let active: std::collections::HashSet<Uuid> = org_frontline_members::Entity::find()
        .filter(org_frontline_members::Column::OrgId.eq(org_id))
        .filter(org_frontline_members::Column::UserId.is_in(ids.clone()))
        .filter(org_frontline_members::Column::Status.eq(org_frontline_members::STATUS_ACTIVE))
        .all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|m| m.user_id)
        .collect();
    let names: std::collections::HashMap<Uuid, String> = users::Entity::find()
        .filter(users::Column::Id.is_in(ids))
        .all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|u| (u.id, u.name))
        .collect();

    rows.into_iter()
        .filter(|c| active.contains(&c.user_id))
        .filter_map(|c| {
            names.get(&c.user_id).map(|name| {
                (
                    c.user_id,
                    RosterEntry {
                        identifier: c.identifier,
                        name: name.clone(),
                    },
                )
            })
        })
        .collect()
}

/// The rule the picker exists to enforce: a kiosk at a place shows the people
/// assigned to that place.
///
/// Pure, and given the assignment rows as `(user_id, location_id)` pairs, so
/// the rule can be tested without a database. It is the whole point of the
/// read and is one deleted line away from being org-wide again.
///
/// `None` for `location` is a kiosk the admin enrolled without a place —
/// today's org-wide behaviour, kept rather than treated as "nowhere".
/// `Some(_)` matches on the location EXACTLY: an assignment carrying
/// `location_id IS NULL` is an org-wide position and does not put its holder
/// on a store's tablet.
///
/// Generic over what rides along with each user id, because the sign-in asks
/// the same question of one person carrying nothing ([`kiosk_admits`]).
fn narrow_to_location<T>(
    candidates: Vec<(Uuid, T)>,
    location: Option<Uuid>,
    assignments: &[(Uuid, Option<Uuid>)],
) -> Vec<T> {
    let Some(location) = location else {
        return candidates.into_iter().map(|(_, entry)| entry).collect();
    };
    let here: std::collections::HashSet<Uuid> = assignments
        .iter()
        .filter(|(_, at)| *at == Some(location))
        .map(|(user, _)| *user)
        .collect();
    candidates
        .into_iter()
        .filter(|(user, _)| here.contains(user))
        .map(|(_, entry)| entry)
        .collect()
}

/// Who of `candidates` belongs on this kiosk: the ONE rule the picker keeps
/// names by and the sign-in admits by, so the two cannot drift apart.
///
/// [`narrow_to_location`] over the scope's own assignment pairs, with a closed
/// scope answering nobody before that rule is consulted. `Nobody` is only ever
/// read at a place, but the guard means a kiosk with no place could never turn
/// an empty or failed read into the whole org.
fn on_this_kiosk<T>(
    candidates: Vec<(Uuid, T)>,
    scope: &RosterScope,
    location: Option<Uuid>,
) -> Vec<T> {
    if *scope == RosterScope::Nobody {
        return Vec::new();
    }
    narrow_to_location(candidates, location, &scope_assignments(scope, location))
}

/// May `user` sign in on this kiosk? [`on_this_kiosk`] asked about one person,
/// so the door answers exactly as the picker does.
fn kiosk_admits(scope: &RosterScope, location: Option<Uuid>, user: Uuid) -> bool {
    !on_this_kiosk(vec![(user, ())], scope, location).is_empty()
}

/// What the door answered, before `login` decides what it is worth.
#[derive(Debug)]
enum KioskVerdict {
    /// The credential layer's answer: admitted, or refused and charged as a
    /// wrong PIN is.
    Pin(PinVerdict),
    /// Nobody is rostered at this kiosk's store, so nobody signs in on it. No
    /// credential was read and none was charged.
    NobodyHere,
}

/// Verify a PIN typed on this kiosk, admitting exactly the people its name
/// picker shows.
///
/// **One rule for the picker and the door.** The scope is [`roster_scope`],
/// the read `roster` narrows the picker with, and admission is
/// [`kiosk_admits`], the rule the picker keeps names by. So a kiosk at a store
/// signs in that store's crew; a kiosk with no place signs in the org, as its
/// picker lists the org; and a store with nobody rostered signs in nobody, as
/// its picker shows nobody. The picker's [`ROSTER_LIMIT`] is not shared: it
/// sizes a screen. The scope read's own ceiling IS shared, so a store past
/// [`STORE_ROSTER_SCAN_LIMIT`] refuses the people its truncation dropped —
/// arbitrary ones, not the late sorters — and says so in a `warn`.
///
/// **A failed scope read is ours, not the worker's.** It returns `Err`, which
/// `login` answers `503` without charging anything. Folded into "nobody" it was
/// a charged refusal of every right PIN at the store, so a blip on that one
/// query left the crew locked out for the lockout window after the database
/// had recovered. The failure depends on neither the identifier nor the PIN, so
/// answering it differently tells a caller nothing about either.
///
/// **No roaming allowance.** An org-wide position (`org_role_members.location_id
/// IS NULL`) is on no store's picker, so it signs in on no store's tablet — on
/// a kiosk with no place it is on the picker and signs in. If roaming staff are
/// wanted, the allowance belongs in `roster_scope` / `narrow_to_location`,
/// where the picker and the sign-in move together.
///
/// **Refused as a wrong PIN is.** The rule is handed INTO
/// [`frontline::verify_pin_admitting`] rather than checked after it, so a right
/// PIN at the wrong store is charged against the same lockout budget, is not
/// stamped as used, and comes back a failed verdict that `login` answers through
/// the wrong-PIN branch: the org brake, the same 401, the same bytes. The scope
/// is read BEFORE the PIN on every attempt, so both cost the same work in the
/// same order. Otherwise anyone who knew a person's PIN could learn from the
/// tablet which stores they work at.
///
/// **A store with nobody rostered charges nobody.** [`RosterScope::Nobody`] is
/// a fact about the KIOSK: every attempt there is refused, whatever identifier
/// and PIN are typed, so there is no person for the refusal to say anything
/// about and no guess for a charge to slow down. Charged, it was a trap. A
/// tablet enrolled before its store's crew were assigned is a normal pre-go-live
/// state, and each right PIN typed at it was a failed attempt, so five of them
/// locked the worker out at their own store too. So it answers
/// [`KioskVerdict::NobodyHere`] before any credential is read, for EVERY
/// identifier alike (no lookup to be fast or slow on), after paying the verify's
/// Argon2 cost ([`frontline::burn_verify_time`]); `login` still counts it
/// against the org brake and sends the wrong-PIN bytes.
async fn verify_at_kiosk(
    db: &sea_orm::DatabaseConnection,
    org_id: Uuid,
    location: Option<Uuid>,
    identifier: &str,
    pin: &str,
) -> Result<KioskVerdict, OxyError> {
    let scope = roster_scope(db, org_id, location)
        .await
        .map_err(|e| OxyError::DBError(format!("kiosk roster scope: {e}")))?;
    if scope == RosterScope::Nobody {
        frontline::burn_verify_time(pin);
        return Ok(KioskVerdict::NobodyHere);
    }
    frontline::verify_pin_admitting(db, org_id, identifier, pin, PinPolicy::default(), |user| {
        kiosk_admits(&scope, location, user)
    })
    .await
    .map(KioskVerdict::Pin)
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub org: String,
    pub identifier: String,
    pub pin: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub name: String,
    /// Seconds until the session expires — so a kiosk can show "signs out at
    /// 23:00" rather than discovering it mid-submission.
    pub expires_in: i64,
}

/// The `Set-Cookie` for a shift session.
///
/// Lifted out of `login` so the handler stays near the ~30-line guidance in
/// `crates/app/CLAUDE.md`, and because both decisions below are ones a reader
/// needs to see together rather than buried in a response builder.
fn shift_session_headers(token: &str, req_headers: &HeaderMap) -> HeaderMap {
    let mut out = HeaderMap::new();
    // The same session cookie the web app uses, so an installed PWA carries it
    // on navigation without the page having to hold the token itself — and built
    // from the same two decisions the magic-link path makes, rather than by a
    // second rule that happens to agree in production.
    //
    // `Secure` from the REQUEST, not from the serve mode. A dev box is cloud
    // mode with non-prod secrets served over plain `http://localhost`, so
    // `!process_is_local()` set `Secure`, the browser discarded the cookie, and
    // kiosk sign-in returned 200 without sticking while magic-link login on the
    // same box worked. `is_request_secure` also honours `X-Forwarded-Proto`,
    // which matters behind an ingress terminating TLS with neither env var set.
    //
    // Max-Age from SHIFT_HOURS, not the 30-day default. The shift TTL was only
    // enforced by the JWT `exp`, so the browser kept a dead cookie for the rest
    // of that window — the morning after a shift the kiosk looked signed in and
    // 401'd on every call instead of showing the name picker.
    let secure = super::auth::is_request_secure(req_headers);
    if let Ok(v) = header::HeaderValue::from_str(&super::auth::build_session_cookie_with_max_age(
        token,
        secure,
        SHIFT_HOURS * 3600,
    )) {
        out.insert(header::SET_COOKIE, v);
    }
    out
}

/// Exchange a PIN for a shift session.
///
/// The session is an ordinary Oxy JWT with a 12-hour expiry. It is narrow not
/// because the token says so but because the AUTHZ MODEL makes it narrow: a
/// frontline worker holds `org_frontline_members` standing, which
/// `oxy_authz::Ring::AppAccess` reads only when ANDed with an explicit
/// `app_members` grant. They reach the apps they were enrolled to use and
/// nothing else — no org read, no workspace, no settings.
///
/// That is worth stating because the alternative was tempting: a bespoke
/// token type, with a bespoke validation path, and a second place for an
/// authorization bug to live.
#[instrument(skip_all, fields(org = %body.org, identifier = %body.identifier))]
pub async fn login(req_headers: HeaderMap, body: Json<LoginRequest>) -> impl IntoResponse {
    let Json(body) = body;

    let Ok(db) = establish_connection().await else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE);
    };

    let Ok(Some(org)) = organizations::Entity::find()
        .filter(organizations::Column::Slug.eq(&body.org))
        .one(&db)
        .await
    else {
        // THE BRAKE COMES FIRST. Closing the timing channel means paying the
        // Argon2 cost on this branch too — `Argon2::default()` is RFC 9106's
        // second profile, m = 19456 KiB and t = 2 — and this is the one path
        // through `login` that `org_is_rate_limited` below cannot cover, because
        // it has no org id to key on. Burning unmetered here would trade a
        // timing oracle for something strictly worse: ~19 MiB and two Argon2
        // passes per request, unauthenticated, at whatever concurrency the
        // caller picks. So the unknown-slug path gets its own bucket, and ends
        // up with exactly the same cost profile as the known-slug path.
        if org_is_rate_limited(NO_SUCH_ORG) {
            return refuse(StatusCode::TOO_MANY_REQUESTS);
        }
        record_org_attempt(NO_SUCH_ORG);

        // Same refusal as a wrong PIN, and the same COST. Matching bodies is
        // only half of it: a known slug goes on to `verify_pin`, which always
        // pays the verify, so returning after one indexed SELECT left the two
        // branches an order of magnitude apart in latency.
        //
        // Load-bearing, not belt-and-braces. `roster` used to disclose org
        // existence by content (`{"staff": []}` for an unknown slug, a list for
        // a known one), which made this branch redundant; now that `roster`
        // answers `{"staff": []}` to anyone without this org's kiosk cookie,
        // this is the only place an unauthenticated caller could have told a
        // real slug from an invented one — and it does not.
        oxy_auth::frontline::burn_verify_time(&body.pin);
        return refuse(StatusCode::UNAUTHORIZED);
    };

    if org_is_rate_limited(org.id) {
        warn!(org_id = %org.id, "frontline login rate-limited for this org");
        // 429 rather than 401: this one IS worth telling the caller apart,
        // because a kiosk should back off rather than retry, and the fact
        // leaked ("somebody is guessing at this org") is not the roster.
        return refuse(StatusCode::TOO_MANY_REQUESTS);
    }

    // The device before the PIN. A PIN is only ever verified for a request
    // that proved it comes from one of THIS org's enrolled kiosks
    // (`frontline_devices`); typed anywhere else it is refused exactly as a
    // wrong PIN is — same status, same body, same cost, and it counts against
    // the org's attempt budget — so a client without a kiosk cookie learns
    // nothing about whether the identifier exists. This is the binding the
    // design record required before any of this faced a user.
    let device = match super::frontline_devices::bound_device(&db, &req_headers).await {
        Some(d) if d.org_id == org.id => d,
        _ => {
            record_org_attempt(org.id);
            oxy_auth::frontline::burn_verify_time(&body.pin);
            info!(org_id = %org.id, "frontline login refused: no kiosk bound to this org");
            return refuse(StatusCode::UNAUTHORIZED);
        }
    };

    // WHO may sign in here is who this tablet's picker shows — see
    // `verify_at_kiosk` for why that check lives inside the verify.
    let verdict =
        match verify_at_kiosk(&db, org.id, device.location_id, &body.identifier, &body.pin).await {
            Ok(v) => v,
            // Ours, not the caller's — a failed roster read included — so
            // nothing is charged, and a tablet backs off instead of having the
            // worker retype.
            Err(e) => {
                warn!(error = %e, "frontline verify failed");
                return refuse(StatusCode::SERVICE_UNAVAILABLE);
            }
        };

    let KioskVerdict::Pin(PinVerdict::Ok { user_id }) = verdict else {
        record_org_attempt(org.id);
        // One response for every failure — wrong PIN, locked out, no such
        // worker, malformed, not rostered at this kiosk's store, nobody
        // rostered at it. `PinVerdict::public_message` exists for exactly this
        // and the difference stays in the log.
        info!(verdict = ?verdict, location = ?device.location_id, "frontline login refused");
        return refuse(StatusCode::UNAUTHORIZED);
    };

    let Ok(Some(user)) = users::Entity::find_by_id(user_id).one(&db).await else {
        return refuse(StatusCode::UNAUTHORIZED);
    };
    let name = user.name.clone();

    let token =
        match super::auth::create_auth_token_with_ttl(user, chrono::Duration::hours(SHIFT_HOURS))
            .await
        {
            Ok(t) => t,
            Err(status) => return refuse(status),
        };

    info!(%user_id, org_id = %org.id, device = %device.id, "frontline session opened");
    super::frontline_devices::touch(&db, device.id).await;

    let out_headers = shift_session_headers(&token, &req_headers);

    (
        out_headers,
        Json(LoginResponse {
            token,
            name,
            expires_in: SHIFT_HOURS * 3600,
        }),
    )
        .into_response()
}

/// Every refusal, in one shape.
fn refuse(status: StatusCode) -> axum::response::Response {
    (
        status,
        Json(serde_json::json!({ "error": "that PIN did not match" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cookie has to die with the token it carries.
    ///
    /// It did not: `build_session_cookie` hardcoded seven days while the shift
    /// JWT expires in twelve hours, so the browser kept presenting a dead
    /// credential for another six days. That does not read as "signed out" — the
    /// kiosk looks signed in and 401s on every call.
    #[test]
    fn the_session_cookie_expires_with_the_shift() {
        let cookie =
            super::super::auth::build_session_cookie_with_max_age("tok", true, SHIFT_HOURS * 3600);
        assert!(
            cookie.contains(&format!("Max-Age={}", SHIFT_HOURS * 3600)),
            "cookie outlives the token: {cookie}"
        );
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Secure"));
    }

    /// `Secure` comes from the request, not the serve mode. A dev box is cloud
    /// mode over plain http, so a mode-derived flag set `Secure` there and the
    /// browser silently dropped the cookie.
    #[test]
    fn an_insecure_request_gets_a_cookie_the_browser_will_keep() {
        let cookie = super::super::auth::build_session_cookie_with_max_age("tok", false, 3600);
        assert!(
            !cookie.contains("Secure"),
            "a plain-http kiosk would discard this: {cookie}"
        );
    }

    /// One entry, named after the person, so the assertions below read as
    /// "who is on this tablet" rather than as index arithmetic.
    fn worker(name: &str) -> (Uuid, RosterEntry) {
        (
            Uuid::new_v4(),
            RosterEntry {
                identifier: name.to_lowercase(),
                name: name.to_string(),
            },
        )
    }

    fn names(entries: &[RosterEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.name.as_str()).collect()
    }

    /// The bug this closes: a kiosk bolted to Clovis listed every PIN holder
    /// in the org, so a Santa Rosa manager appeared on the Clovis tablet.
    #[test]
    fn a_kiosk_at_a_store_shows_only_that_stores_crew() {
        let clovis = Uuid::new_v4();
        let santa_rosa = Uuid::new_v4();
        let (maria, maria_entry) = worker("Maria");
        let (dev, dev_entry) = worker("Devon");
        let candidates = vec![(maria, maria_entry), (dev, dev_entry)];
        let assignments = vec![(maria, Some(clovis)), (dev, Some(santa_rosa))];

        assert_eq!(
            names(&narrow_to_location(
                candidates.clone(),
                Some(clovis),
                &assignments
            )),
            ["Maria"],
            "the tablet's own store is the roster"
        );
        // Same rows, other tablet — proving the filter reads the location it
        // was given rather than just dropping everyone but the first row.
        assert_eq!(
            names(&narrow_to_location(
                candidates,
                Some(santa_rosa),
                &assignments
            )),
            ["Devon"]
        );
    }

    /// An org-wide position is not a position at every store. An Account
    /// Manager signs in on the web; putting them on every tablet in the chain
    /// is the org-wide picker again under another name.
    #[test]
    fn an_org_wide_position_is_not_on_a_stores_tablet() {
        let clovis = Uuid::new_v4();
        let (maria, maria_entry) = worker("Maria");
        let (amy, amy_entry) = worker("Amy");
        let staff = narrow_to_location(
            vec![(maria, maria_entry), (amy, amy_entry)],
            Some(clovis),
            &[(maria, Some(clovis)), (amy, None)],
        );
        assert_eq!(names(&staff), ["Maria"]);
    }

    /// A worker with no assignment at all is nobody's crew — including on a
    /// tablet at a store they have never been rostered to.
    #[test]
    fn an_unrostered_worker_is_on_no_stores_tablet() {
        let clovis = Uuid::new_v4();
        let (ghost, ghost_entry) = worker("Ghost");
        assert!(
            narrow_to_location(vec![(ghost, ghost_entry)], Some(clovis), &[]).is_empty(),
            "an empty assignment table must not fall through to org-wide"
        );
    }

    /// A kiosk enrolled without a place keeps today's behaviour: there is no
    /// store to narrow to, and a picker nobody can load is a kiosk nobody can
    /// use.
    #[test]
    fn a_kiosk_with_no_place_still_shows_the_org() {
        let (maria, maria_entry) = worker("Maria");
        let (dev, dev_entry) = worker("Devon");
        let staff = narrow_to_location(
            vec![(maria, maria_entry), (dev, dev_entry)],
            None,
            // Assignments exist and must be ignored, not consulted.
            &[(maria, Some(Uuid::new_v4()))],
        );
        assert_eq!(names(&staff), ["Maria", "Devon"]);
    }

    /// The identifier order the query asked for is what people learn to tap.
    #[test]
    fn narrowing_keeps_the_order_the_query_chose() {
        let clovis = Uuid::new_v4();
        let (a, a_entry) = worker("Ana");
        let (b, b_entry) = worker("Bo");
        let (c, c_entry) = worker("Cy");
        let staff = narrow_to_location(
            vec![(a, a_entry), (b, b_entry), (c, c_entry)],
            Some(clovis),
            &[(c, Some(clovis)), (a, Some(clovis))],
        );
        assert_eq!(
            names(&staff),
            ["Ana", "Cy"],
            "the assignment rows' order must not reorder the picker"
        );
    }

    /// The bug this closes: the 200-row cap was applied org-wide and the store
    /// filter ran on the survivors, so in a tenant past 200 PIN credentials a
    /// store whose crew sorts late lost them from its own picker — with no
    /// error, no empty list, the worker simply not there.
    ///
    /// The property is that the narrowing and the cap are in the SAME
    /// statement, so the cap counts this store's people. Asserting on the built
    /// SQL is asserting on the read the route actually issues: `roster_body`
    /// runs this very query.
    #[test]
    fn the_row_cap_counts_the_stores_people_not_the_tenants() {
        use sea_orm::QueryTrait;
        let maria = Uuid::new_v4();
        let sql = credential_query(Uuid::new_v4(), &RosterScope::Store(vec![maria]))
            .build(sea_orm::DbBackend::Postgres)
            .to_string();

        let narrowed_at = sql
            .find(&maria.to_string())
            .unwrap_or_else(|| panic!("the store's people are not in the credential read: {sql}"));
        let capped_at = sql
            .rfind("LIMIT")
            .unwrap_or_else(|| panic!("the credential read is not capped: {sql}"));
        assert!(
            narrowed_at < capped_at,
            "the cap must apply to the narrowed read, not to the tenant: {sql}"
        );
        assert!(
            sql.contains(&format!("LIMIT {ROSTER_LIMIT}")),
            "the cap the picker is sized for must reach the database: {sql}"
        );
    }

    /// The guard on the test above: prove the `IN` is not simply always there.
    ///
    /// It also pins the behaviour a placeless kiosk keeps — there is no store
    /// to narrow to, so the cap is org-wide, exactly as it has always been.
    #[test]
    fn a_kiosk_with_no_place_reads_the_org() {
        use sea_orm::QueryTrait;
        let sql = credential_query(Uuid::new_v4(), &RosterScope::Org)
            .build(sea_orm::DbBackend::Postgres)
            .to_string();
        assert!(
            !sql.contains(" IN ("),
            "a kiosk with no place has nothing to narrow to: {sql}"
        );
        assert!(
            sql.contains(&format!("LIMIT {ROSTER_LIMIT}")),
            "the org-wide read is still capped: {sql}"
        );
    }

    /// The two halves compose: what the scope narrowed the READ to is what
    /// `narrow_to_location` then keeps. A candidate the scope never asked for
    /// — a stale row, a widened query — is still dropped.
    #[test]
    fn the_scope_is_the_store_rule_narrow_to_location_enforces() {
        let clovis = Uuid::new_v4();
        let (maria, maria_entry) = worker("Maria");
        let (dev, dev_entry) = worker("Devon");
        let scope = RosterScope::Store(vec![maria]);

        let staff = narrow_to_location(
            vec![(maria, maria_entry), (dev, dev_entry)],
            Some(clovis),
            &scope_assignments(&scope, Some(clovis)),
        );
        assert_eq!(names(&staff), ["Maria"]);
    }

    /// A store with nobody rostered is an empty picker — and so is a store whose
    /// assignment read failed, which `roster_body` answers as this same scope.
    /// Neither may fall through to the tenant's crew, so neither carries
    /// assignment pairs for the rule to keep.
    #[test]
    fn a_closed_scope_carries_nobody() {
        let clovis = Uuid::new_v4();
        assert!(scope_assignments(&RosterScope::Nobody, Some(clovis)).is_empty());
        let (ghost, ghost_entry) = worker("Ghost");
        assert!(
            narrow_to_location(
                vec![(ghost, ghost_entry)],
                Some(clovis),
                &scope_assignments(&RosterScope::Nobody, Some(clovis)),
            )
            .is_empty(),
            "a failed assignment read must not widen the picker to the tenant"
        );
    }

    /// The door the picker's rule now guards: a store's tablet signs in its own
    /// crew, and a right PIN from another store is refused there.
    #[test]
    fn a_store_kiosk_admits_its_own_crew_and_nobody_elses() {
        let clovis = Uuid::new_v4();
        let (maria, _) = worker("Maria");
        let (dev, _) = worker("Devon");
        // What `roster_scope` reads for Clovis: its own people, by location
        // exactly — so neither Devon (Santa Rosa) nor an org-wide position.
        let scope = RosterScope::Store(vec![maria]);
        assert!(kiosk_admits(&scope, Some(clovis), maria));
        assert!(
            !kiosk_admits(&scope, Some(clovis), dev),
            "a worker not rostered at this store must not sign in on its tablet"
        );
    }

    /// A kiosk with no place lists the org, so it admits the org.
    #[test]
    fn a_kiosk_with_no_place_admits_the_org() {
        let (anyone, _) = worker("Anyone");
        assert!(kiosk_admits(&RosterScope::Org, None, anyone));
    }

    /// A closed scope — nobody rostered — admits nobody, wherever the tablet
    /// is. The placeless half is the one the guard in `on_this_kiosk` exists
    /// for: `narrow_to_location` alone would read a missing place as "the whole
    /// org". (A FAILED read never becomes this scope at the door; it is an
    /// uncharged 503 — `frontline_kiosk_signin` pins that against a real
    /// database.)
    #[test]
    fn a_closed_scope_admits_nobody() {
        let (maria, _) = worker("Maria");
        assert!(!kiosk_admits(
            &RosterScope::Nobody,
            Some(Uuid::new_v4()),
            maria
        ));
        assert!(
            !kiosk_admits(&RosterScope::Nobody, None, maria),
            "a closed scope must never fall through to the org"
        );
    }

    #[test]
    fn the_org_brake_opens_and_closes() {
        let org = Uuid::new_v4();
        assert!(!org_is_rate_limited(org), "a fresh org is not limited");
        for _ in 0..ORG_ATTEMPT_CEILING {
            record_org_attempt(org);
        }
        assert!(
            org_is_rate_limited(org),
            "the ceiling must actually stop a caller walking the roster — the \
             per-credential lockout cannot, because 40 names is 40 budgets"
        );
        // A different org is unaffected: the brake is per tenant, so one store
        // under attack cannot lock out another.
        assert!(!org_is_rate_limited(Uuid::new_v4()));
    }

    #[tokio::test]
    async fn every_refusal_carries_the_same_body() {
        // The status differs (429 tells a kiosk to back off) but the body must
        // not, or the response distinguishes "no such worker" from "wrong PIN".
        //
        // READ the body. The first version of this test `Debug`-formatted an
        // unread `axum::body::Body`, which prints the same opaque placeholder
        // for every response — so it compared three identical strings and could
        // not fail. It would have passed with three different bodies, which is
        // the only thing it was written to catch.
        let mut bodies = Vec::new();
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::SERVICE_UNAVAILABLE,
        ] {
            let bytes = axum::body::to_bytes(refuse(status).into_body(), 64 * 1024)
                .await
                .expect("refusal bodies are small and always present");
            bodies.push(String::from_utf8(bytes.to_vec()).expect("utf-8"));
        }
        assert!(
            !bodies[0].is_empty(),
            "a refusal with an empty body would make this vacuous again"
        );
        assert!(
            bodies.windows(2).all(|w| w[0] == w[1]),
            "refusals differ and leak which layer refused: {bodies:?}"
        );
    }

    /// The guard on the test above: prove the comparison can actually fail.
    ///
    /// Without this, a future edit that reverts `refuse` to something opaque
    /// makes the assertion vacuous again with nothing to notice.
    #[tokio::test]
    async fn the_refusal_body_comparison_can_fail() {
        let a = axum::body::to_bytes(refuse(StatusCode::UNAUTHORIZED).into_body(), 64 * 1024)
            .await
            .unwrap();
        let b = axum::body::to_bytes(
            (StatusCode::UNAUTHORIZED, Json(serde_json::json!({"x": 1})))
                .into_response()
                .into_body(),
            64 * 1024,
        )
        .await
        .unwrap();
        assert_ne!(a, b, "two genuinely different bodies compared equal");
    }
}

// ── Enrolment ───────────────────────────────────────────────────────────────

/// No `Debug`. This struct holds a raw PIN, and a derived `Debug` is one
/// `tracing` field or one `unwrap` panic away from putting it in a log line.
#[derive(Deserialize)]
pub struct EnrolRequest {
    /// Shown on the kiosk's name picker. Not unique — two Marias are two rows.
    pub name: String,
    /// What the worker picks themselves out by. Unique per org.
    pub identifier: String,
    /// 4–8 digits. Never stored, never logged, never returned.
    pub pin: String,
    /// The apps this worker will use, by id — granted `member` in the same
    /// call. Optional: a manager can also grant later from an app's access
    /// settings, which accept an active worker. Every id must be this org's,
    /// or the whole request is refused before the worker exists.
    #[serde(default)]
    pub apps: Vec<Uuid>,
    /// Where they work, and as what — positions at places, written after the
    /// worker exists and checked before. Optional: a manager can also roster
    /// them later from Settings.
    #[serde(default)]
    pub assignments: Vec<AssignmentSpec>,
}

/// Enrol a frontline worker — the door `enroll_worker` never had.
///
/// # Why this is the missing piece
///
/// Everything else in this file already shipped: the PIN credential, the
/// standing row, the login exchange, the roster read. `oxy_auth::frontline::
/// enroll_worker` has existed the whole time with **zero non-test callers**, so
/// `GET /api/frontline/roster` has been answering `200 {"staff": []}` on every
/// deployment — a read path with no write path, which looks exactly like a
/// tenant that has not enrolled anybody.
///
/// # Who may call it
///
/// `OrgAdmin`, which is the guard the rest of the org's member management uses.
/// Deliberately NOT a new authorization concept: enrolling a worker is adding a
/// person to an org, and inventing a second rule for it is how two answers to
/// one question start disagreeing. A store manager who is not an org admin
/// cannot enrol yet — that is a real gap, and it is one for the roles model to
/// close rather than for this route to route around.
///
/// # What it deliberately does not do
///
/// No email, no invitation, no `org_members` row. That is the whole design:
/// `enroll_worker` writes `users.email = NULL`, which keeps this person out of
/// every email-keyed path — OAuth collapse, Slack matching, invitations,
/// platform grants — by construction rather than by a check somebody has to
/// remember. The worker exists, can sign in, and holds nothing else.
#[instrument(skip_all, fields(org = %org_id))]
pub async fn enrol(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path(org_id): Path<Uuid>,
    Json(req): Json<EnrolRequest>,
) -> impl IntoResponse {
    let Ok(db) = establish_connection().await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "database unavailable" })),
        )
            .into_response();
    };

    // The apps first, before a worker exists to be left half set up. Deciding
    // an app's audience is `AppAccessManage` — the ring the access settings
    // enforce — and `OrgAdmin` is not that ring, so a request that grants is
    // held to it here; then an id that is not this org's refuses the whole
    // request.
    let apps = super::frontline_grants::normalize_app_ids(req.apps);
    if !apps.is_empty()
        && !super::frontline_grants::may_grant_apps(
            &db,
            actor.id,
            actor.email.as_deref().unwrap_or(""),
            org_id,
        )
        .await
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({
                "error": "granting apps at enrollment needs the standing to manage app access"
            })),
        )
            .into_response();
    }
    if let Err(e) = super::frontline_grants::validate_apps_in_org(&db, org_id, &apps).await {
        return match e {
            super::frontline_grants::GrantError::NotThisOrg(_) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
            super::frontline_grants::GrantError::Db(err) => {
                error!(%org_id, "frontline enrolment: app lookup failed: {err}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": "enrollment failed" })),
                )
                    .into_response()
            }
        };
    }

    // The positions next, still before the worker exists: a position that is
    // not this org's, or a store for an org-wide position, refuses the whole
    // request rather than leaving a person half rostered.
    for spec in &req.assignments {
        if let Err(e) = assignments::validate_targets(&db, org_id, spec).await {
            return (
                e.status(),
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    }

    match frontline::enroll_worker(
        &db,
        org_id,
        &req.name,
        &req.identifier,
        &req.pin,
        PinPolicy::default(),
    )
    .await
    {
        Ok(user_id) => {
            info!(%org_id, %user_id, apps = apps.len(), "frontline worker enrolled");
            // The grants, now that there is somebody to grant to. Validated
            // above, so the only way this fails is the database — and then the
            // worker exists without their apps, which the response must say
            // rather than report a clean 201.
            let granted = match super::frontline_grants::grant_apps_to_worker(
                &db,
                org_id,
                user_id,
                &apps,
                Some(actor.id),
            )
            .await
            {
                Ok(granted) => granted,
                Err(e) => {
                    error!(%org_id, %user_id, "worker enrolled, but the app grants failed: {e}");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "the worker was enrolled but could not be granted their apps \
                                      — grant them from each app's access settings",
                            "user_id": user_id,
                        })),
                    )
                        .into_response();
                }
            };
            // Gaining access is as auditable as losing it: the same entry the
            // access settings file, one per app, so this door is not the one
            // way to reach an app that leaves no trail.
            for app in &granted {
                super::org_teams::audit::record(
                    &db,
                    &ctx,
                    &actor,
                    super::org_teams::audit::APP_ACCESS_CHANGED,
                    (
                        "app",
                        app.id,
                        format!(
                            "{} ({}) ← enrolled {}",
                            app.name,
                            app.slug,
                            req.identifier.trim()
                        ),
                    ),
                )
                .await;
            }
            // Then the roster. Validated above, so again only the database
            // can fail this — and then the worker exists with their apps but
            // not their positions, which the response says.
            let rostered = match assignments::roster_at_enrolment(
                &db,
                org_id,
                user_id,
                &req.assignments,
                &actor,
            )
            .await
            {
                Ok(ids) => ids,
                Err(e) => {
                    error!(%org_id, %user_id, "worker enrolled, but rostering failed: {e}");
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": "the worker was enrolled but could not be rostered \
                                      — add their positions from Settings → Crew",
                            "user_id": user_id,
                        })),
                    )
                        .into_response();
                }
            };
            // The PIN is not echoed. An admin who did not keep it re-enrols or
            // resets; a response that repeats it would put it in every proxy
            // log between here and the browser.
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "user_id": user_id,
                    "identifier": req.identifier.trim(),
                    "name": req.name.trim(),
                    "apps": apps,
                    "assignments": rostered,
                })),
            )
                .into_response()
        }
        // Match the VARIANT, not every error.
        //
        // `enroll_worker` validates the PIN policy and the required fields, and
        // those messages are exactly what an admin needs — so a validation
        // failure is a 400 carrying its own sentence.
        //
        // Everything else is ours. Mapping them all to 400 told an admin that a
        // pool exhaustion was their bad request, and put raw database text
        // ("enrol begin: …") in the response body on the way. A 500 with a
        // generic body is the honest answer; the real error goes to the log,
        // where it belongs.
        Err(OxyError::ValidationError(msg)) => {
            warn!(%org_id, "frontline enrolment refused: {msg}");
            // 409 for a taken identifier, 400 for a malformed request.
            //
            // Re-enrolling somebody who already exists, or reusing a badge
            // number, is the most likely way this call fails and is entirely
            // the admin's to fix — it is a conflict with existing state, not a
            // bad request. Matched on `frontline::IDENTIFIER_TAKEN` rather than
            // on a literal, so the two sides cannot drift apart silently.
            let status = if msg == frontline::IDENTIFIER_TAKEN {
                StatusCode::CONFLICT
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({ "error": msg }))).into_response()
        }
        Err(e) => {
            error!(%org_id, error = %e, "frontline enrolment failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "could not enroll the worker" })),
            )
                .into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct StandingRequest {
    /// `false` suspends, `true` reinstates.
    pub active: bool,
}

/// Suspend a frontline worker, or reinstate one.
///
/// The other half of enrolment, and it was missing: a worker could be enrolled
/// and never un-enrolled. The `status` column has modelled `suspended` since the
/// schema landed and nothing wrote it, so the door opened one way — which I
/// found by enrolling a test worker into a demo org and having no way to remove
/// them.
///
/// `PATCH`, not `DELETE`, because nothing is deleted. A worker who leaves keeps
/// their row so the work they did stays attributed; suspension is what takes
/// away the ability to sign in. `verify_pin` and the roster read the same
/// column, so one write closes the door on both the login and the name picker.
///
/// Idempotent. Suspending an already-suspended worker answers 200 with
/// `changed: false` rather than 409 — the caller asked for a state and that is
/// the state, and making a retry look like a conflict is how a client learns to
/// ignore the status code.
#[instrument(skip_all, fields(org = %org_id, worker = %user_id))]
pub async fn set_standing(
    OrgAdmin(ctx): OrgAdmin,
    AuthenticatedUserExtractor(actor): AuthenticatedUserExtractor,
    Path((org_id, user_id)): Path<(Uuid, Uuid)>,
    Json(req): Json<StandingRequest>,
) -> impl IntoResponse {
    let Ok(db) = establish_connection().await else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "database unavailable" })),
        )
            .into_response();
    };

    match frontline::set_worker_standing(&db, org_id, user_id, req.active).await {
        Ok(changed) => {
            // "Who cut this worker off, and when."
            //
            // The neighbouring route in this same router writes
            // `org.member.removed` under a comment reading "Losing access is as
            // auditable as gaining it" — and suspension IS losing access. This
            // route shipped with a `tracing::info!` carrying no actor at all, so
            // once logs rolled the answer was unrecoverable.
            //
            // Only on a real change: an idempotent no-op is not an event, and an
            // audit log that records every retry is one nobody reads.
            //
            // Best-effort, like its neighbour: failing to write the trail must
            // not fail a revocation that has already happened. A worker whose
            // access was removed but whose removal went unlogged is bad; leaving
            // their access in place because the logging failed is worse.
            if changed {
                // `user_can_access_app` caches its verdict per (user, app) for
                // the app shell and every function invoke; a suspension that
                // left that entry warm would keep the kiosk working until it
                // expired. Same call every grant-changing route makes.
                crate::server::api::custom_apps_auth::invalidate_access_cache();
                let action = if req.active {
                    "frontline.worker.reinstated"
                } else {
                    "frontline.worker.suspended"
                };
                audit::record_best_effort(
                    &db,
                    audit::AuditEntry::new(actor.label().to_string(), action)
                        .actor(actor.id, audit::ActorType::User)
                        .org(ctx.org.id)
                        .target("frontline_worker", user_id.to_string(), String::new())
                        .change(
                            serde_json::json!({ "active": !req.active }),
                            serde_json::json!({ "active": req.active }),
                        ),
                )
                .await;
            }
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "user_id": user_id,
                    "active": req.active,
                    // What the statement DID, not what was asked for. The two differ
                    // exactly when the worker was already in that state, and a
                    // caller reconciling a roster needs to tell those apart.
                    "changed": changed,
                })),
            )
                .into_response()
        }
        // Same split as enrolment: the admin's mistake carries its sentence,
        // everything else is ours and says nothing about the database.
        Err(OxyError::ValidationError(msg)) => {
            warn!(%org_id, "frontline standing refused: {msg}");
            // 404 for THIS refusal, matched by name — 400 for any other.
            //
            // Blanket-mapping the variant was right while the writer had exactly
            // one validation, and would have quietly reported the next one — a
            // bad status, a self-suspend guard — as "worker not found". Same
            // reasoning as `enrol` matching `IDENTIFIER_TAKEN` rather than a
            // literal: the two sides agree on a name so they cannot drift.
            let status = if msg == frontline::WORKER_NOT_FOUND {
                StatusCode::NOT_FOUND
            } else {
                StatusCode::BAD_REQUEST
            };
            (status, Json(serde_json::json!({ "error": msg }))).into_response()
        }
        Err(e) => {
            error!(%org_id, error = %e, "frontline standing failed");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": "could not change the worker's standing" })),
            )
                .into_response()
        }
    }
}
