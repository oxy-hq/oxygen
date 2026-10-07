//! The **only** reader of Oxy's two platform-standing sources.
//!
//! `OXY_OWNER` (an env allow-list) and the `app_admins` table answer one question:
//! is this person Oxy staff, and how senior? That is authorization input, and it used to
//! be read directly from ~20 call sites across handlers, middlewares, the partner tier
//! and the assume path — each re-deciding what "staff" means and what it grants. Two of
//! them combined the flags differently for no stated reason.
//!
//! So the primitives are read here and nowhere else, and callers take one of two doors:
//!
//! * A **decision** — `authz::allows(&facts, Action::Platform*, &Resource::platform())`.
//!   The ring says what staff may do; the call site doesn't restate it.
//! * A **flag to display** — [`platform_standing`], for payloads like `/me` that report
//!   `is_owner` / `is_app_admin` and decide nothing.
//!
//! Keeping both behind this module is what stops the third pattern — a handler
//! hand-rolling `is_oxy_owner() || is_app_admin()` and quietly inventing a policy.
//!
//! ## Every door takes a [`Caller`], not an address
//!
//! Standing is stored by email, but it is *held* by a credential: an API token
//! with `platform = false` holds none, and one narrowed to a few orgs holds it
//! over those orgs only (API-tokens design §4.4). A door keyed by a bare email
//! cannot know that, so each one takes the [`Caller`] and answers for the
//! standing **as that credential carries it**. A browser session and a legacy
//! key carry all of it, unchanged. The two address-keyed reads,
//! [`grant_of_email`] and [`staff_holding`], are for a *subject* — someone a
//! staff console is looking at, an address a notification goes to — never for
//! the requester.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

use entity::prelude::AppAdmins;
use entity::{app_admin_scope_orgs, app_admins};
use oxy_authz::{PlatformRole, PlatformStanding as Grant, Scope};
use sea_orm::{ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter};

use crate::caller::Caller;

/// What Oxy's platform sources say about a person, **as flags to display**. Not a
/// decision, and deliberately lossy: it says *that* someone is staff, never what they
/// may do. Feed [`platform_grant_checked`] to a ring when you need the latter.
///
/// Renamed off `PlatformStanding` when platform standing became a real grant — that
/// name now belongs to [`oxy_authz::PlatformStanding`], which carries the capabilities
/// and scope. Two types with one name, one of them a boolean pair, is how a call site
/// ends up deciding access from a display flag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlatformFlags {
    /// In the `OXY_OWNER` env allow-list.
    pub is_global_owner: bool,
    /// Holds a row in the platform-grant table (`app_admins`) — **any** role. A
    /// display flag only: an App Operator and a Global Admin both report `true` here,
    /// which is precisely why nothing may authorize from it.
    pub is_global_admin: bool,
}

impl PlatformFlags {
    /// Either flag — "is this Oxy staff at all". The `oxy_owner_or_app_admin` shape.
    pub fn is_staff(self) -> bool {
        self.is_global_owner || self.is_global_admin
    }
}

/// Is the caller Oxy's root, **as this credential carries it**? The owner allow-list
/// is an env read with no DB, so sync callers that need only this half don't have to
/// become async to go through the front door.
///
/// A token without `platform` is not root, and neither is one narrowed to a list of
/// orgs — root is unbounded by definition (`TokenReach::narrow_platform`).
pub fn is_global_owner(caller: &Caller) -> bool {
    let listed = crate::oxy_owner_guard::is_oxy_owner(caller.standing_email());
    listed && caller.reach().is_none_or(|r| r.all_access)
}

/// The platform sources for `caller`, narrowed to what the credential carries.
/// `listed` and `grant` are what the sources say about the *address*.
fn carried(caller: &Caller, listed: bool, grant: Option<Grant>) -> (bool, Option<Grant>) {
    match caller.reach() {
        None => (listed, grant),
        Some(reach) => reach.narrow_platform(listed, grant),
    }
}

/// TTL for the `app_admins` membership cache. Matches the 60s the check used before it
/// moved here from `custom_apps_auth`.
const ADMIN_CACHE_TTL: Duration = Duration::from_secs(60);

/// Cache of the platform **grant** for an email — `None` meaning "looked, not staff".
/// Self-contained here (rather than reusing `custom_apps_auth`'s cache helper) so authz
/// owns its only `app_admins` read with **no** import back into `custom_apps_*` — that
/// import was a dependency cycle blocking the customer-apps surface from moving.
///
/// The cache holds the whole grant, not a bool, so the role and scope ride the same
/// entry the membership check already paid for. A second cache keyed differently would
/// be a way for "is staff" and "what may they do" to disagree for up to a TTL.
type GrantCache = RwLock<HashMap<String, (Option<Grant>, Instant)>>;

fn admin_cache() -> &'static GrantCache {
    static CACHE: OnceLock<GrantCache> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn cached_admin(email: &str) -> Option<Option<Grant>> {
    let cache = admin_cache().read().ok()?;
    let (value, at) = cache.get(email)?;
    (at.elapsed() < ADMIN_CACHE_TTL).then(|| value.clone())
}

fn set_cached_admin(email: String, grant: Option<Grant>) {
    if let Ok(mut cache) = admin_cache().write() {
        // Sweep expired entries so churn of distinct emails can't grow the map unbounded.
        cache.retain(|_, (_, at)| at.elapsed() < ADMIN_CACHE_TTL);
        cache.insert(email, (grant, Instant::now()));
    }
}

/// Drop every cached `app_admins` verdict. Callers on the write side — the admin
/// grant/revoke endpoints and the env bootstrap — invalidate after mutating the table so
/// a freshly granted admin isn't masked by a stale cached `false` for up to the TTL.
pub fn invalidate_admin_cache() {
    if let Ok(mut cache) = admin_cache().write() {
        cache.clear();
    }
}

/// Does the caller hold a platform grant row (`app_admins`), as this credential
/// carries it? `Err` is a lookup failure, distinct from a `false` verdict —
/// [`platform_standing_checked`] is what decides how that unknown collapses.
///
/// Moved here from `custom_apps_auth` so authz owns this read outright; the only other
/// caller is `oxy_app_admin_guard`.
pub async fn is_app_admin(db: &DatabaseConnection, caller: &Caller) -> Result<bool, DbErr> {
    Ok(platform_grant_checked(db, caller).await?.is_some())
}

/// The **authorization** read: the caller's platform grant as their credential carries
/// it, or `None` if it carries none. A Global Owner on a token narrowed to a list of
/// orgs reads as a Global Admin over those orgs here — see [`is_global_owner`].
pub async fn platform_grant_checked(
    db: &DatabaseConnection,
    caller: &Caller,
) -> Result<Option<Grant>, DbErr> {
    if !caller.carries_platform() {
        return Ok(None);
    }
    if caller.is_sandbox_agent() {
        return uncached_grant(db, caller).await;
    }
    let grant = grant_of_email(db, caller.standing_email()).await?;
    let listed = crate::oxy_owner_guard::is_oxy_owner(caller.standing_email());
    Ok(carried(caller, listed, grant).1)
}

/// [`platform_grant_checked`] for a **sandbox agent token**: its minter's
/// grant read now, past the 60 s cache, and kept on the caller for the rest
/// of the request (`grant_memo`).
///
/// "When the minter loses access the token stops at once" is decided here,
/// inside the one door every guard takes, so no call site has to remember to
/// ask for a fresh read (sandbox agent credential design §4). A failed read
/// is not kept: the next guard asks again.
async fn uncached_grant(db: &DatabaseConnection, caller: &Caller) -> Result<Option<Grant>, DbErr> {
    if let Some(read) = caller.grant_memo().get() {
        return Ok(read);
    }
    let grant = fresh_standing(db, caller).await?.grant;
    caller.grant_memo().set(grant.clone());
    Ok(grant)
}

/// The platform grant stored for an **address**, or `None` if there is none. Cached
/// for [`ADMIN_CACHE_TTL`] alongside the membership check.
///
/// This is what the sources say about a person, not what a request may do: it knows
/// nothing of the credential a request arrived with. Use it for a *subject* — a row a
/// staff console is displaying or editing. For the requester, take
/// [`platform_grant_checked`].
///
/// Two rules make an unreadable grant deny rather than escalate:
///
/// * a `role` this build cannot expand ([`PlatformRole::from_str`] returns `None`) drops
///   the whole grant — so rolling back past a role's introduction removes standing
///   instead of reinterpreting it as something more powerful;
/// * `scope_all = false` yields `Scope::Orgs`, which reaches nothing when the child
///   table is empty. Unbounded reach is never inferred from missing rows.
pub async fn grant_of_email(db: &DatabaseConnection, email: &str) -> Result<Option<Grant>, DbErr> {
    let key = email.trim().to_ascii_lowercase();
    if key.is_empty() {
        return Ok(None);
    }
    if let Some(v) = cached_admin(&key) {
        return Ok(v);
    }
    read_grant(db, key).await
}

/// The platform grant stored for `key` (a normalized address), read from the
/// table and written back to the cache. The one statement of how a row becomes
/// a grant — the cached read and the fresh one both end here.
async fn read_grant(db: &DatabaseConnection, key: String) -> Result<Option<Grant>, DbErr> {
    let Some(row) = AppAdmins::find()
        .filter(app_admins::Column::Email.eq(key.clone()))
        .one(db)
        .await?
    else {
        set_cached_admin(key, None);
        return Ok(None);
    };

    let Some(role) = PlatformRole::from_str(&row.role) else {
        tracing::warn!(
            target: "authz",
            role = %row.role,
            "platform grant names a role this build cannot expand — dropping the grant"
        );
        set_cached_admin(key, None);
        return Ok(None);
    };

    let scope = if row.scope_all {
        Scope::All
    } else {
        Scope::Orgs(
            app_admin_scope_orgs::Entity::find()
                .filter(app_admin_scope_orgs::Column::AppAdminId.eq(row.id))
                .all(db)
                .await?
                .into_iter()
                .map(|s| s.org_id)
                .collect(),
        )
    };

    let grant = Grant::from_role(role, scope);
    set_cached_admin(key, Some(grant.clone()));
    Ok(Some(grant))
}

/// A staff address and where its standing reaches — for **notifying** staff, never for
/// deciding what a request may do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StaffAddress {
    pub email: String,
    pub scope: Scope,
    /// The role the standing was granted as; `None` for the Global Owner, who is root
    /// by the allow-list and holds no grant.
    pub role: Option<PlatformRole>,
}

/// Every address whose stored standing holds `cap`: the `OXY_OWNER` allow-list (root, so
/// unbounded) and each `app_admins` grant whose role expands to it.
///
/// About addresses, like [`grant_of_email`], and the one enumeration of the two sources —
/// here so a job that emails staff does not read them itself. Uncached: its callers are
/// periodic jobs, not requests.
///
/// A grant's scope travels with the address because the caller must narrow what it sends:
/// a grant bounded to two orgs is told about those two.
pub async fn staff_holding(
    db: &DatabaseConnection,
    cap: oxy_authz::Cap,
) -> Result<Vec<StaffAddress>, DbErr> {
    let rows = AppAdmins::find().all(db).await?;
    let bounded: Vec<uuid::Uuid> = rows.iter().filter(|r| !r.scope_all).map(|r| r.id).collect();
    let mut orgs_of: HashMap<uuid::Uuid, Vec<uuid::Uuid>> = HashMap::new();
    if !bounded.is_empty() {
        let scopes = app_admin_scope_orgs::Entity::find()
            .filter(app_admin_scope_orgs::Column::AppAdminId.is_in(bounded))
            .all(db)
            .await?;
        for s in scopes {
            orgs_of.entry(s.app_admin_id).or_default().push(s.org_id);
        }
    }
    let grants = rows.into_iter().map(|row| {
        let scope = if row.scope_all {
            Scope::All
        } else {
            Scope::Orgs(orgs_of.remove(&row.id).unwrap_or_default())
        };
        (row.email, row.role, scope)
    });
    Ok(staff_from(
        crate::oxy_owner_guard::oxy_owner_emails(),
        grants,
        cap,
    ))
}

/// [`staff_holding`] without the reads. Owners first; an address in both sources is
/// listed once, as root. A role this build cannot expand holds nothing — the rule
/// [`grant_of_email`] applies to a single grant.
fn staff_from(
    owners: Vec<String>,
    grants: impl IntoIterator<Item = (String, String, Scope)>,
    cap: oxy_authz::Cap,
) -> Vec<StaffAddress> {
    let mut staff: Vec<StaffAddress> = Vec::new();
    for email in owners {
        if !staff.iter().any(|s| s.email == email) {
            staff.push(StaffAddress {
                email,
                scope: Scope::All,
                role: None,
            });
        }
    }
    for (email, role, scope) in grants {
        let email = email.trim().to_ascii_lowercase();
        let Some(role) = PlatformRole::from_str(&role) else {
            continue;
        };
        if email.is_empty() || staff.iter().any(|s| s.email == email) {
            continue;
        }
        if Grant::from_role(role, scope.clone()).holds(cap) {
            staff.push(StaffAddress {
                email,
                scope,
                role: Some(role),
            });
        }
    }
    staff
}

/// The caller's platform standing **read now**, past the 60 s grant cache: for a
/// decision that must not outlive a grant change by a cache window.
///
/// The one use today is minting a sandbox agent token, whose who-may-mint check is
/// stated as uncached (sandbox agent credential design §2). It is deliberately not
/// an address-keyed read beside the cached one: like every door here it takes the
/// [`Caller`], so a narrowed credential is answered for the standing it carries.
#[derive(Clone, Debug)]
pub struct FreshStanding {
    is_global_owner: bool,
    grant: Option<Grant>,
}

impl FreshStanding {
    /// Does the caller hold `cap` over `org_id`? As [`platform_reaches`]: root
    /// short-circuits, and a grant must name the capability and reach the org.
    pub fn reaches(&self, cap: oxy_authz::Cap, org_id: uuid::Uuid) -> bool {
        self.is_global_owner || self.grant.as_ref().is_some_and(|g| g.grants(cap, org_id))
    }

    /// Whether the caller holds any staff standing at all.
    pub fn is_staff(&self) -> bool {
        self.is_global_owner || self.grant.is_some()
    }

    /// Where this standing reaches at all: every org for root and for an
    /// unbounded grant, the grant's orgs for a bounded one, and nowhere for no
    /// standing. For narrowing a query to the orgs worth asking about —
    /// [`Self::reaches`] is still what decides each row.
    pub fn scope(&self) -> Scope {
        if self.is_global_owner {
            return Scope::All;
        }
        self.grant
            .as_ref()
            .map_or_else(|| Scope::Orgs(Vec::new()), |grant| grant.scope.clone())
    }
}

/// Read [`FreshStanding`] for the caller. `Err` is a failed lookup, which a
/// caller deciding access must treat as no standing.
pub async fn fresh_standing(
    db: &DatabaseConnection,
    caller: &Caller,
) -> Result<FreshStanding, DbErr> {
    let grant = if caller.carries_platform() {
        let key = caller.standing_email().trim().to_ascii_lowercase();
        let stored = if key.is_empty() {
            None
        } else {
            read_grant(db, key).await?
        };
        let listed = crate::oxy_owner_guard::is_oxy_owner(caller.standing_email());
        carried(caller, listed, stored).1
    } else {
        None
    };
    Ok(FreshStanding {
        is_global_owner: is_global_owner(caller),
        grant,
    })
}

/// **Does the caller hold `cap` over `org_id`?** The platform tier's org-scoped question,
/// for call sites that resolve an actor rather than enforce a ring.
///
/// Reach for this instead of `platform_standing(..).is_staff()` anywhere the answer
/// implies authority *inside a tenant*. `is_staff()` is now true for every platform
/// role, so it can no longer distinguish an App Operator from a Global Admin, and it
/// never consulted scope at all — a grant bounded to org A would pass it for org B.
///
/// Handlers that enforce a ring don't need this: `allows()` already applies the same
/// rule via `PrincipalFacts::platform_grants`. This exists for the resolve-an-actor
/// shape (publish authority, assume-role, app serving), where there is no ring to lean
/// on and a bare `is_staff()` silently voids scope.
///
/// A Global Owner short-circuits — root holds no grant row. An unreadable grant denies.
///
/// A token carries this only into the orgs it covers: one narrowed to org A does not
/// reach org B, whatever its bearer's scope.
pub async fn platform_reaches(
    db: &DatabaseConnection,
    caller: &Caller,
    cap: oxy_authz::Cap,
    org_id: uuid::Uuid,
) -> bool {
    if is_global_owner(caller) {
        return true;
    }
    matches!(
        platform_grant_checked(db, caller).await,
        Ok(Some(grant)) if grant.grants(cap, org_id)
    )
}

/// **Does the caller hold `cap` at all?** Scope is not consulted — the platform-surface
/// question, matching `Ring::PlatformCap`.
///
/// Use for surfaces that belong to Oxy rather than to a tenant (the partner registry,
/// the console sections). Where the question is reach *into a specific org*, use
/// [`platform_reaches`] instead so the grant's scope applies.
pub async fn platform_holds(db: &DatabaseConnection, caller: &Caller, cap: oxy_authz::Cap) -> bool {
    if is_global_owner(caller) {
        return true;
    }
    matches!(
        platform_grant_checked(db, caller).await,
        Ok(Some(grant)) if grant.holds(cap)
    )
}

/// Read the platform sources for the caller, distinguishing **"not staff"** from **"we
/// could not find out"**. `None` is the latter: the `app_admins` lookup errored, so no
/// verdict here is honest.
///
/// That distinction only matters to a decision, which is why it is the door the loader
/// takes. Collapsing an errored lookup to `false` is safe in isolation — it withholds
/// standing rather than inventing it — but under `enforce` it is read as a *fact* that
/// the principal is not staff, and the model then subtracts access their legacy check
/// granted. A wrong 403, from a blip.
pub async fn platform_standing_checked(
    db: &DatabaseConnection,
    caller: &Caller,
) -> Option<Checked> {
    match platform_grant_checked(db, caller).await {
        Ok(grant) => Some(Checked {
            flags: PlatformFlags {
                is_global_owner: is_global_owner(caller),
                is_global_admin: grant.is_some(),
            },
            grant,
        }),
        Err(e) => {
            tracing::warn!(
                target: "authz",
                error = %e,
                "app_admins lookup failed — platform standing is unknown, not absent"
            );
            None
        }
    }
}

/// A resolved platform read: the display flags **and** the grant behind them.
///
/// They travel together because they are one database read and must never disagree.
/// The loader takes [`Self::grant`] (what may this person do); `/me`-style payloads take
/// [`Self::flags`] (is this person staff). A call site that authorizes from `flags` has
/// re-created the boolean this whole change removed — take the grant.
#[derive(Clone, Debug)]
pub struct Checked {
    pub flags: PlatformFlags,
    pub grant: Option<Grant>,
}

/// The most standing that can be established with **no database**: the `OXY_OWNER`
/// allow-list, which is an env read.
///
/// This is not a lesser `Default`. `PlatformFlags::default()` says "no standing";
/// this says "no standing *we needed the database for*" — and the difference is a Global
/// Owner keeping their owner-tier UI through a DB outage. Owner status never depended on
/// the DB, so no DB failure should be able to take it away.
pub fn platform_standing_offline(caller: &Caller) -> PlatformFlags {
    PlatformFlags {
        is_global_owner: is_global_owner(caller),
        // Genuinely unknown without the `app_admins` table. Withheld, not invented.
        is_global_admin: false,
    }
}

/// Read the platform sources for the caller. The `app_admins` lookup is cached and the
/// owner check is an env read, so this is cheap enough for a per-request payload.
///
/// Fail-closed **only where it has to be**: an unresolvable `app_admins` lookup reports
/// no admin standing rather than granting it, but the owner half falls back to
/// [`platform_standing_offline`] rather than collapsing with it. That is the right
/// behaviour for a **flag to display** (`/me`) and for a call site whose own check reads
/// these same sources. If you are feeding a ring, take [`platform_standing_checked`] and
/// decide for yourself what unknown means.
pub async fn platform_standing(db: &DatabaseConnection, caller: &Caller) -> PlatformFlags {
    for_display(
        platform_standing_checked(db, caller).await.map(|c| c.flags),
        caller,
    )
}

/// How an unknown standing collapses for display. Split out from [`platform_standing`]
/// only so it is reachable without a database — this one line IS the bug that motivated
/// the split (`unwrap_or_default()` here silently un-owned a Global Owner), so it should
/// be pinned by a test rather than reviewed by eye.
fn for_display(known: Option<PlatformFlags>, caller: &Caller) -> PlatformFlags {
    known.unwrap_or_else(|| platform_standing_offline(caller))
}

#[cfg(test)]
#[path = "globals_sandbox_tests.rs"]
mod sandbox_tests;

#[cfg(test)]
mod tests {
    #[test]
    fn a_blank_email_holds_no_platform_standing() {
        // Every caller that lost a guaranteed address to frontline identity now
        // passes `user.email.as_deref().unwrap_or("")` into here. This is the
        // choke point that makes that safe: blank is nobody. `is_oxy_owner`
        // has the matching test for the allow-list side.
        let flags = super::platform_standing_offline(&super::Caller::without_credential(
            uuid::Uuid::nil(),
            "",
        ));
        assert!(!flags.is_global_owner);
        assert!(!flags.is_global_admin);
    }

    use super::*;

    #[test]
    fn staff_holding_a_capability_is_root_plus_the_roles_that_expand_to_it() {
        use oxy_authz::Cap;
        let org = uuid::Uuid::from_u128(9);
        let grants = || {
            vec![
                (
                    "Admin@oxy.tech ".to_string(),
                    "global_admin".to_string(),
                    Scope::All,
                ),
                (
                    "bounded@oxy.tech".to_string(),
                    "global_admin".to_string(),
                    Scope::Orgs(vec![org]),
                ),
                (
                    "operator@oxy.tech".to_string(),
                    "app_operator".to_string(),
                    Scope::All,
                ),
                (
                    "root@oxy.tech".to_string(),
                    "global_admin".to_string(),
                    Scope::Orgs(vec![]),
                ),
                (
                    "future@oxy.tech".to_string(),
                    "a_role_from_later".to_string(),
                    Scope::All,
                ),
            ]
        };
        let owners = || vec!["root@oxy.tech".to_string()];

        let operate = staff_from(owners(), grants(), Cap::OperatePlatform);
        let emails: Vec<&str> = operate.iter().map(|s| s.email.as_str()).collect();
        // An App Operator does not operate the platform; an unknown role holds nothing.
        assert_eq!(
            emails,
            ["root@oxy.tech", "admin@oxy.tech", "bounded@oxy.tech"]
        );
        // Root is listed once, and unbounded whatever its grant row says.
        assert_eq!(operate[0].scope, Scope::All);
        assert_eq!(operate[0].role, None, "root holds no grant");
        assert_eq!(operate[1].role, Some(PlatformRole::GlobalAdmin));
        assert_eq!(operate[2].scope, Scope::Orgs(vec![org]));

        let apps = staff_from(owners(), grants(), Cap::ManageApps);
        assert!(apps.iter().any(|s| s.email == "operator@oxy.tech"));
    }

    fn session(email: &str) -> Caller {
        Caller::without_credential(uuid::Uuid::from_u128(1), email)
    }

    /// A caller on a new-format token with these flags and one org-wide grant.
    fn token(email: &str, all_access: bool, platform: bool) -> Caller {
        use oxy_auth::token::{CredentialContext, StoredKind};
        let user = oxy_auth::types::AuthenticatedUser {
            id: uuid::Uuid::from_u128(1),
            email: Some(email.to_string()),
            name: "t".into(),
            picture: None,
            status: entity::users::UserStatus::Active,
            credential: None,
        };
        let credential = CredentialContext {
            token_id: uuid::Uuid::from_u128(2),
            kind: StoredKind::Personal,
            principal_user_id: user.id,
            all_access,
            platform,
            partner: true,
            name: "t".into(),
            display_prefix: "oxy_pat_Ab3x".into(),
            legacy_api_key_id: None,
            blocked_orgs: Vec::new(),
            expires_at: None,
            service_account: None,
            grants: vec![oxy_authz::TokenGrant {
                org_id: uuid::Uuid::from_u128(7),
                workspace_id: None,
                ceiling: oxy_authz::RoleCeiling::Owner,
            }],
            app_publish: Vec::new(),
            app_sandbox: Vec::new(),
        };
        Caller::of(&user, Some(&credential))
    }

    /// The owner allow-list is an env read, so this half of "a token holds only the
    /// standing it carries" is provable with no database.
    #[test]
    #[serial_test::serial(oxy_owner_env)]
    fn a_token_is_root_only_when_it_is_all_access_and_carries_platform() {
        unsafe { std::env::set_var("OXY_OWNER", "owner@oxy.tech") };
        let session_is_root = is_global_owner(&session("owner@oxy.tech"));
        let unrestricted = is_global_owner(&token("owner@oxy.tech", true, true));
        let no_platform = is_global_owner(&token("owner@oxy.tech", true, false));
        let bounded = is_global_owner(&token("owner@oxy.tech", false, true));
        let offline = platform_standing_offline(&token("owner@oxy.tech", true, false));
        unsafe { std::env::remove_var("OXY_OWNER") };

        assert!(session_is_root);
        assert!(
            unrestricted,
            "an all-access token with platform is its bearer"
        );
        assert!(!no_platform, "platform=false holds no standing");
        assert!(
            !bounded,
            "root is unbounded; a bounded token cannot be root"
        );
        assert_eq!(offline, PlatformFlags::default());
    }

    /// `carried` is the whole narrowing of the grant half, and it is pure.
    #[test]
    fn a_token_carries_the_grant_only_as_far_as_it_reaches() {
        let all = Grant::from_role(PlatformRole::GlobalAdmin, Scope::All);
        let org = uuid::Uuid::from_u128(7);
        let other = uuid::Uuid::from_u128(8);

        let (root, grant) = carried(&session("a@oxy.tech"), false, Some(all.clone()));
        assert!(!root);
        assert_eq!(
            grant,
            Some(all.clone()),
            "a session carries the grant whole"
        );

        let (_, grant) = carried(&token("a@oxy.tech", true, false), true, Some(all.clone()));
        assert_eq!(grant, None, "platform=false carries none");

        let (root, grant) = carried(&token("a@oxy.tech", false, true), true, Some(all.clone()));
        let grant = grant.expect("a bounded root token carries a bounded grant");
        assert!(!root);
        assert!(grant.grants(oxy_authz::Cap::ManageOrgSettings, org));
        assert!(!grant.grants(oxy_authz::Cap::ManageOrgSettings, other));

        let (_, grant) = carried(&token("a@oxy.tech", false, true), false, Some(all));
        assert_eq!(grant.unwrap().scope, Scope::Orgs(vec![org]));
    }

    /// The `app_admins` cache moved here with `is_app_admin_email` so that authz no
    /// longer reaches into `custom_apps_auth` for it (that import was a cycle). A
    /// cache that dropped writes would re-query every call — correctness-neutral but
    /// the point of the cache — so pin the round-trip.
    ///
    /// It now stores the whole GRANT rather than a bool, so the round-trip has to
    /// prove the role and scope survive too — a cache that dropped them would answer
    /// "is staff" correctly while silently widening an App Operator to whatever the
    /// re-derived default was.
    #[test]
    fn admin_cache_round_trips_a_stored_grant() {
        let email = "admin-cache-probe@oxy.tech";
        assert_eq!(cached_admin(email), None, "a cold cache misses");

        let scoped = Grant::from_role(
            PlatformRole::AppOperator,
            Scope::Orgs(vec![uuid::Uuid::from_u128(7)]),
        );
        set_cached_admin(email.to_string(), Some(scoped.clone()));
        assert_eq!(
            cached_admin(email),
            Some(Some(scoped)),
            "a warm cache returns the stored grant — role and scope included — not a re-query"
        );
    }

    /// "Looked, and they are not staff" must cache as a hit, or every anonymous-ish
    /// request re-queries `app_admins`.
    #[test]
    fn admin_cache_stores_a_negative_verdict_as_a_hit() {
        let email = "not-staff-probe@example.com";
        set_cached_admin(email.to_string(), None);
        assert_eq!(
            cached_admin(email),
            Some(None),
            "a cached 'not staff' is a hit, not a miss"
        );
    }

    /// The regression this exists to prevent: a DB outage must not un-own an owner.
    ///
    /// Both halves used to collapse together onto `PlatformFlags::default()`, which
    /// reported `is_owner: false` at a Global Owner and hid their own UI — over a
    /// failure in a table their standing never depended on.
    #[test]
    #[serial_test::serial(oxy_owner_env)]
    fn offline_standing_keeps_the_owner_flag_it_never_needed_a_database_for() {
        unsafe { std::env::set_var("OXY_OWNER", "owner@oxy.tech") };
        let standing = platform_standing_offline(&session("owner@oxy.tech"));
        unsafe { std::env::remove_var("OXY_OWNER") };

        assert!(
            standing.is_global_owner,
            "owner standing is an env read — no database failure is a reason to drop it"
        );
        assert!(
            !standing.is_global_admin,
            "admin standing is genuinely unknown without app_admins; withhold it, don't invent it"
        );
    }

    /// The other half: "no DB" must not become a grant.
    #[test]
    #[serial_test::serial(oxy_owner_env)]
    fn offline_standing_grants_nothing_to_a_non_owner() {
        unsafe { std::env::set_var("OXY_OWNER", "owner@oxy.tech") };
        let standing = platform_standing_offline(&session("someone.else@example.com"));
        unsafe { std::env::remove_var("OXY_OWNER") };

        assert_eq!(
            standing,
            PlatformFlags::default(),
            "a caller not on the allow-list has no standing to report offline"
        );
    }

    /// The wiring, not just the helper. This is the assertion that actually fails if
    /// someone writes `unwrap_or_default()` — which is exactly the regression that
    /// shipped, and which a test of `platform_standing_offline` alone would sail past.
    #[test]
    #[serial_test::serial(oxy_owner_env)]
    fn an_unknown_standing_falls_back_to_the_env_not_to_default() {
        unsafe { std::env::set_var("OXY_OWNER", "owner@oxy.tech") };
        let unknown = for_display(None, &session("owner@oxy.tech"));
        let known = for_display(
            Some(PlatformFlags {
                is_global_owner: true,
                is_global_admin: true,
            }),
            &session("owner@oxy.tech"),
        );
        unsafe { std::env::remove_var("OXY_OWNER") };

        assert!(
            unknown.is_global_owner,
            "an app_admins failure must not cost an owner the flag the env already proves"
        );
        assert!(!unknown.is_global_admin, "admin standing stays withheld");
        assert!(
            known.is_global_admin,
            "a known standing must pass through untouched, not be re-derived offline"
        );
    }
}
