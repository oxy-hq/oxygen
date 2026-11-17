//! Shared TTL caches for the customer-apps hot paths.
//!
//! The bundle-serve handler (`custom_apps_serve`), the function/health/debug
//! prelude (`custom_apps_auth::authenticate_and_authorize`) and the data-plane
//! gate (`custom_apps_gates`) run the same per-request resolution chain —
//! authenticate, look up user, resolve the app or workspace, decide access —
//! for every asset request and every product fetch. A single page load is
//! 30-100 asset requests + N parallel product fetches; without caching,
//! that's hundreds of DB hits per click.
//!
//! Every cache here is one [`TtlLru`]: a hard capacity, least-recently-read
//! eviction, and a 60 s per-entry TTL. The TTL is intentionally short so that
//! a membership revocation propagates within a minute **without** explicit
//! invalidation; the invalidation hooks below exist for the mutations that
//! must land at once (a publish, a visibility change, a rename). That bound
//! is the bundle path's. The data-plane gate memoises only what cannot
//! over-admit — the ring's subtracting term — and reads the workspace row
//! and the shipped gate fresh on every request, so a revoked membership or
//! a moved workspace is refused by the next data call on every replica.
//!
//! A **miss is never cached** by any of them: a newly created app, user or
//! workspace is reachable immediately.

use std::sync::OnceLock;
use std::time::Duration;

use oxy_auth::types::AuthenticatedUser;
use uuid::Uuid;

pub use oxy_app_core::ttl_lru::TtlLru;

/// Lifetime of cache entries before they need re-fetching. Short enough
/// that org membership revocations propagate quickly; long enough to
/// absorb the asset-storm a page load triggers.
pub const CACHE_TTL: Duration = Duration::from_secs(60);

/// Hard cap per cache. The entry evicted past it is the least recently read,
/// so a scan over many identities or slugs costs the scanner's own cold
/// entries, not the hot ones.
pub const CACHE_MAX_ENTRIES: usize = 4_096;

/// A cache of the standard shape: `CACHE_MAX_ENTRIES` slots, `CACHE_TTL`.
pub fn standard_cache<K: std::hash::Hash + Eq, V: Clone>() -> TtlLru<K, V> {
    TtlLru::new(CACHE_MAX_ENTRIES, CACHE_TTL)
}

// ── User cache ──────────────────────────────────────────────────────────────
//
// Keyed by `custom_apps_auth::user_cache_key`: the user id a session names,
// and the lowercased address only for a provider identity that names nobody
// yet. It was keyed by the email string, which is "" for every frontline
// worker — one slot for the whole crew. See that function for the story.
//
// One entry per live identity, which under the old unbounded sweep was the
// one map that could grow without limit; it is now the bound above.

fn user_cache() -> &'static TtlLru<String, AuthenticatedUser> {
    static CACHE: OnceLock<TtlLru<String, AuthenticatedUser>> = OnceLock::new();
    CACHE.get_or_init(standard_cache)
}

pub fn cached_user(key: &str) -> Option<AuthenticatedUser> {
    user_cache().get(&key.to_string())
}

pub fn set_cached_user(key: String, user: AuthenticatedUser) {
    user_cache().insert(key, user);
}

// ── Access verdicts ((user_id, app_id) → may open the app) ──────────────────
//
// The combined (member | grant | staff) decision `custom_apps_auth::
// user_can_access_app` makes, kept so an asset storm doesn't ask it three
// times per chunk. The verdict of a SESSION: a token that narrows its bearer
// neither reads nor writes it (`custom_apps_auth`).

fn access_cache() -> &'static TtlLru<(Uuid, Uuid), bool> {
    static CACHE: OnceLock<TtlLru<(Uuid, Uuid), bool>> = OnceLock::new();
    CACHE.get_or_init(standard_cache)
}

pub(crate) fn cached_access(user_id: Uuid, app_id: Uuid) -> Option<bool> {
    access_cache().get(&(user_id, app_id))
}

pub(crate) fn set_cached_access(user_id: Uuid, app_id: Uuid, allowed: bool) {
    access_cache().insert((user_id, app_id), allowed);
}

/// Drop every access verdict on this replica. The hook behind
/// `custom_apps_auth::invalidate_access_cache`, which is what the mutation
/// sites call; also drops the data-plane gate's cached ring terms, which
/// weigh on the same question for a workspace.
pub fn clear_access_caches() {
    access_cache().clear();
    model_term_cache().clear();
    no_app_role_cache().clear();
    no_memo_standing_cache().clear();
}

// ── Data-plane gate: the ring's term ───────────────────────────────────────
//
// `custom_apps_gates::check_custom_app_gates` memoises ONE thing, and not the
// decision: the ring's term — `allows(facts, WorkspaceDataAccess,
// workspace)` — per `(user_id, project_id, org_id)`. The workspace row is read fresh
// on every request (its `org_id` is the tenant boundary the whole decision
// is scoped by, and a move must take effect on every replica at once), and
// so is the shipped gate (membership, frontline grant, staff reach, partner
// reach, lockdown); the decision is `shipped && term`. The term can only
// subtract, so a stale one over-refuses and never over-admits. The staging
// pin is per request by design.
//
// `org_id` is part of the key, read fresh from the workspace row on every
// call: a workspace transfer to another org (or a delete and re-create)
// changes the key, so a term minted under the old org can never be hit —
// no replica-local invalidation hook is needed for the move to take effect.

fn model_term_cache() -> &'static TtlLru<(Uuid, Uuid, Uuid), bool> {
    static CACHE: OnceLock<TtlLru<(Uuid, Uuid, Uuid), bool>> = OnceLock::new();
    CACHE.get_or_init(standard_cache)
}

pub fn cached_model_term(user_id: Uuid, project_id: Uuid, org_id: Uuid) -> Option<bool> {
    model_term_cache().get(&(user_id, project_id, org_id))
}

/// Remember the ring's term — only a term that was actually computed. `None`
/// is "the facts did not load": the gate then deferred to the shipped
/// verdict for that one request, and a verdict reached without facts is
/// never anyone else's.
pub fn remember_model_term(user_id: Uuid, project_id: Uuid, org_id: Uuid, term: Option<bool>) {
    if let Some(term) = term {
        model_term_cache().insert((user_id, project_id, org_id), term);
    }
}

// ── No app role ((user_id, app_id, org_id) → holds none) ────────────────────
//
// `custom_apps_auth::resolve_app_role` reads the caller's standing (five
// reads) and then the app's grants (up to three more) to answer "admin",
// "member" or nothing. Only the last answer is remembered, and only as the
// fact "this user holds no role on this app": an entry can make a role show as
// missing for up to a minute after a grant, and nothing in it can make a role
// appear that the caller does not hold. A role that was found, admin or
// member, is never written, so a revoked admin is read fresh on the next call.
//
// The key carries `org_id` from the app row, read fresh on every call, so an
// app that moves to another org cannot hit an entry made under the old one.
// Dropped by `clear_access_caches`, i.e. by every access-invalidation hook.
// A session's answer only: a token that narrows its bearer neither reads nor
// writes it, since its "no role" is a ceiling's, not the user's.

fn no_app_role_cache() -> &'static TtlLru<(Uuid, Uuid, Uuid), ()> {
    static CACHE: OnceLock<TtlLru<(Uuid, Uuid, Uuid), ()>> = OnceLock::new();
    CACHE.get_or_init(standard_cache)
}

/// True when a resolution within the last minute found this user holds no role
/// on this app.
pub fn cached_no_app_role(user_id: Uuid, app_id: Uuid, org_id: Uuid) -> bool {
    no_app_role_cache()
        .get(&(user_id, app_id, org_id))
        .is_some()
}

/// Remember that the user holds no role on the app. Callers write this only
/// after a resolution that loaded its facts and found neither admin nor a
/// grant; a role is never remembered.
pub fn remember_no_app_role(user_id: Uuid, app_id: Uuid, org_id: Uuid) {
    no_app_role_cache().insert((user_id, app_id, org_id), ());
}

// A user whose "no role" is never remembered — Oxy staff, or a member of an org
// that holds a partner grant, whose admin verdict the fact loader can get wrong
// without saying so (see `auth::resolve_app_role`). Established with a few reads,
// so the answer is kept for the same minute: it only stops those reads being
// repeated on every call, and it can only keep a "no role" from being stored.
// Dropped with the rest by `clear_access_caches`.

fn no_memo_standing_cache() -> &'static TtlLru<Uuid, ()> {
    static CACHE: OnceLock<TtlLru<Uuid, ()>> = OnceLock::new();
    CACHE.get_or_init(standard_cache)
}

/// True when this user was found, within the last minute, to hold a standing for
/// which no-role answers are not remembered.
pub fn cached_no_memo_standing(user_id: Uuid) -> bool {
    no_memo_standing_cache().get(&user_id).is_some()
}

pub fn remember_no_memo_standing(user_id: Uuid) {
    no_memo_standing_cache().insert(user_id, ());
}

// ── App resolution cache ((org_slug, app_slug) → org + app rows) ────────────
//
// The two lookups that turn a URL into rows, cached together because they're
// resolved together and neither is useful without the other. Dropped by the
// `invalidate_app_resolution_cache*` family on any app-row mutation
// (publish, promote, visibility, delete, rename) — the app row carries
// `published_at` and `visibility`, and the entry carries the builds the app's
// environments serve — all of which steer the serve decision, so a stale row
// must not outlive a publish.

/// An `(organizations, apps)` row pair resolved from a URL's slugs, and the
/// builds the app's environments serve (`app_environments`). The environment
/// rows move exactly when the app's pointers do, at the same call sites that
/// already invalidate this cache, so they ride the same entry rather than
/// costing the asset storm a query of their own.
#[derive(Clone)]
pub struct ResolvedApp {
    pub org: entity::organizations::Model,
    pub app: entity::apps::Model,
    pub environments: crate::env_resolve::EnvironmentBuilds,
}

type AppResolutionKey = (String, String);

fn app_resolution_cache() -> &'static TtlLru<AppResolutionKey, ResolvedApp> {
    static CACHE: OnceLock<TtlLru<AppResolutionKey, ResolvedApp>> = OnceLock::new();
    CACHE.get_or_init(standard_cache)
}

pub fn cached_app_resolution(org_slug: &str, app_slug: &str) -> Option<ResolvedApp> {
    app_resolution_cache().get(&(org_slug.to_string(), app_slug.to_string()))
}

pub fn set_cached_app_resolution(org_slug: &str, app_slug: &str, resolved: ResolvedApp) {
    app_resolution_cache().insert((org_slug.to_string(), app_slug.to_string()), resolved);
}

/// Drop the cached resolution of **one** app, by the slugs that key it.
///
/// For a mutation that knows which app it touched — a publish, a promote, a
/// visibility change, a delete — so every other app on the replica keeps its
/// warm entry. A slug rename must pass the **old** slug: the key is what has
/// to go, and the new slug was never cached (a miss is never cached).
pub fn invalidate_app_resolution_cache_for(org_slug: &str, app_slug: &str) {
    app_resolution_cache().remove(&(org_slug.to_string(), app_slug.to_string()));
    org_cache_remove_by_slug(org_slug);
}

/// Drop the cached resolution of **one** app, by its id.
///
/// For a mutation that holds the app row but not the org's slug — a draft
/// publish, a visibility write. The entry is found by value, so a rename
/// that changed the app's slug is dropped too (the row inside still carries
/// the id).
pub fn invalidate_app_resolution_cache_for_app(app_id: Uuid) {
    app_resolution_cache().retain(|_, resolved| resolved.app.id != app_id);
}

/// Drop the cached resolution of **every** app of one org, by its slug.
///
/// For an org-level mutation — a rename (pass the old slug), a delete — where
/// the org row inside every cached pair is what changed.
pub fn invalidate_app_resolution_cache_for_org(org_slug: &str) {
    app_resolution_cache().retain(|(org, _), _| org != org_slug);
    org_cache_remove_by_slug(org_slug);
}

/// Drop every cached slug→rows resolution on this replica.
///
/// The wholesale fallback for a call site that does not know which slugs it
/// affected. Prefer [`invalidate_app_resolution_cache_for`] or
/// [`invalidate_app_resolution_cache_for_org`]: mutations are rare but a
/// replica serving many apps should not re-resolve all of them because one
/// was published.
pub fn invalidate_app_resolution_cache() {
    app_resolution_cache().clear();
    org_cache().clear();
}

// ── Org-by-id cache (organizations by primary key) ──────────────────────────
//
// The shell-context bootstrap reads the owning org by the id the gate
// resolved; it has no slug to reach the resolution cache with. Invalidated
// with that cache, because an org row changes at the same call sites.

fn org_cache() -> &'static TtlLru<Uuid, entity::organizations::Model> {
    static CACHE: OnceLock<TtlLru<Uuid, entity::organizations::Model>> = OnceLock::new();
    CACHE.get_or_init(standard_cache)
}

pub fn cached_org(org_id: Uuid) -> Option<entity::organizations::Model> {
    org_cache().get(&org_id)
}

pub fn set_cached_org(org: entity::organizations::Model) {
    org_cache().insert(org.id, org);
}

fn org_cache_remove_by_slug(org_slug: &str) {
    org_cache().retain(|_, org| org.slug != org_slug);
}

// ── Build-row cache (app_builds by primary key) ─────────────────────────────
//
// The third per-asset query. A build row is written once by the publish
// pipeline and the fields the serve path reads (`build_id`, `s3_prefix`)
// never change afterwards — a promote/rollback repoints `apps`, it does not
// rewrite `app_builds`. Keyed by PK, so a repointed channel looks up a
// different key and cannot serve a stale build.

fn build_cache() -> &'static TtlLru<Uuid, entity::app_builds::Model> {
    static CACHE: OnceLock<TtlLru<Uuid, entity::app_builds::Model>> = OnceLock::new();
    CACHE.get_or_init(standard_cache)
}

pub fn cached_build(build_pk: Uuid) -> Option<entity::app_builds::Model> {
    build_cache().get(&build_pk)
}

pub fn set_cached_build(build_pk: Uuid, build: entity::app_builds::Model) {
    build_cache().insert(build_pk, build);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn org(slug: &str) -> entity::organizations::Model {
        let now = chrono::Utc::now().fixed_offset();
        entity::organizations::Model {
            id: Uuid::new_v4(),
            name: slug.to_string(),
            slug: slug.to_string(),
            logo: None,
            logo_content_type: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// An `apps` row: app `id` of `org_id`, published from `workspace_id`. The
    /// same row `custom_apps_agent_fixture::app` builds in `oxy-app`, whose
    /// fixtures this crate cannot reach.
    fn app(id: Uuid, org_id: Uuid, workspace_id: Uuid) -> entity::apps::Model {
        let now = chrono::Utc::now().fixed_offset();
        entity::apps::Model {
            visibility: "org".to_string(),
            id,
            slug: "ops".to_string(),
            name: "Ops".to_string(),
            org_id,
            project_id: workspace_id,
            branch: "main".to_string(),
            source_repo: String::new(),
            status: "created".to_string(),
            source_type: "s3".to_string(),
            source_config: serde_json::json!({}),
            bootstrap_pr_url: None,
            last_synced_at: None,
            manifest_override: None,
            published_at: None,
            repo_path: None,
            last_promoted_by: None,
            last_promoted_at: None,
            require_second_person_to_promote: false,
            created_at: now,
            updated_at: now,
        }
    }

    fn resolved(org_slug: &str) -> ResolvedApp {
        let org = org(org_slug);
        ResolvedApp {
            app: app(Uuid::new_v4(), org.id, Uuid::new_v4()),
            org,
            environments: Default::default(),
        }
    }

    #[test]
    fn targeted_invalidation_drops_one_app_and_keeps_its_neighbours() {
        // Unique slugs: the cache is process-global and other tests share it.
        let acme = format!("acme-{}", Uuid::new_v4());
        set_cached_app_resolution(&acme, "sales", resolved(&acme));
        set_cached_app_resolution(&acme, "ops", resolved(&acme));
        invalidate_app_resolution_cache_for(&acme, "sales");
        assert!(cached_app_resolution(&acme, "sales").is_none());
        assert!(cached_app_resolution(&acme, "ops").is_some());
    }

    #[test]
    fn app_invalidation_finds_the_entry_by_the_row_it_holds() {
        let acme = format!("acme-{}", Uuid::new_v4());
        let sales = resolved(&acme);
        let sales_id = sales.app.id;
        set_cached_app_resolution(&acme, "sales", sales);
        set_cached_app_resolution(&acme, "ops", resolved(&acme));
        invalidate_app_resolution_cache_for_app(sales_id);
        assert!(cached_app_resolution(&acme, "sales").is_none());
        assert!(cached_app_resolution(&acme, "ops").is_some());
    }

    #[test]
    fn org_invalidation_drops_every_app_of_that_org_only() {
        let acme = format!("acme-{}", Uuid::new_v4());
        let globex = format!("globex-{}", Uuid::new_v4());
        set_cached_app_resolution(&acme, "sales", resolved(&acme));
        set_cached_app_resolution(&acme, "ops", resolved(&acme));
        set_cached_app_resolution(&globex, "sales", resolved(&globex));
        let globex_org = org(&globex);
        let globex_id = globex_org.id;
        set_cached_org(globex_org);
        let acme_org = org(&acme);
        let acme_id = acme_org.id;
        set_cached_org(acme_org);

        invalidate_app_resolution_cache_for_org(&acme);

        assert!(cached_app_resolution(&acme, "sales").is_none());
        assert!(cached_app_resolution(&acme, "ops").is_none());
        assert!(cached_app_resolution(&globex, "sales").is_some());
        assert!(
            cached_org(acme_id).is_none(),
            "the org row goes with its apps"
        );
        assert!(cached_org(globex_id).is_some());
    }

    #[test]
    fn model_terms_fall_with_the_access_verdicts() {
        let user = Uuid::new_v4();
        let project = Uuid::new_v4();
        let org = Uuid::new_v4();
        remember_model_term(user, project, org, Some(true));
        assert_eq!(cached_model_term(user, project, org), Some(true));
        clear_access_caches();
        assert_eq!(cached_model_term(user, project, org), None);
    }

    #[test]
    fn a_no_role_memo_falls_with_the_access_verdicts() {
        let (user, app, org) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        assert!(!cached_no_app_role(user, app, org));
        remember_no_app_role(user, app, org);
        assert!(cached_no_app_role(user, app, org));
        // A different org (the app moved) or user never hits it.
        assert!(!cached_no_app_role(user, app, Uuid::new_v4()));
        assert!(!cached_no_app_role(Uuid::new_v4(), app, org));
        clear_access_caches();
        assert!(!cached_no_app_role(user, app, org));
    }

    #[test]
    fn the_no_memo_standing_falls_with_the_access_verdicts() {
        let user = Uuid::new_v4();
        assert!(!cached_no_memo_standing(user));
        remember_no_memo_standing(user);
        assert!(cached_no_memo_standing(user));
        clear_access_caches();
        assert!(!cached_no_memo_standing(user));
    }

    #[test]
    fn a_transferred_workspace_never_hits_the_old_orgs_term() {
        // The old org's member held a cached `true` for this workspace. After
        // the transfer the gate reads the NEW org id from the workspace row,
        // so the key differs and the stale term is unreachable.
        let user = Uuid::new_v4();
        let project = Uuid::new_v4();
        let old_org = Uuid::new_v4();
        let new_org = Uuid::new_v4();
        remember_model_term(user, project, old_org, Some(true));
        assert_eq!(cached_model_term(user, project, old_org), Some(true));
        assert_eq!(cached_model_term(user, project, new_org), None);
    }

    #[test]
    fn a_term_reached_without_facts_is_never_remembered() {
        // The facts-did-not-load arm defers to the shipped verdict for that
        // one request; remembering it would hand a blip's verdict to the
        // next minute of requests.
        let user = Uuid::new_v4();
        let project = Uuid::new_v4();
        let org = Uuid::new_v4();
        remember_model_term(user, project, org, None);
        assert_eq!(cached_model_term(user, project, org), None);
        remember_model_term(user, project, org, Some(false));
        assert_eq!(cached_model_term(user, project, org), Some(false));
    }
}
