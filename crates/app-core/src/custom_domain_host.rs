//! Customer-owned hostnames for custom apps — `app.acme.com` serving the
//! production environment of one app, exactly as its platform subdomain
//! `<org>--<slug>.customer-apps.<zone>` does.
//!
//! This module is the read side every request touches, so it lives beside the
//! other two host dispatchers ([`crate::custom_apps_host_dispatch`],
//! [`crate::org_host_dispatch`]):
//!
//! - [`normalize_domain`] — the one spelling a hostname is stored and looked up
//!   in, and the refusal of names that can never be a customer's (our own
//!   zones, IP literals, single labels, reserved suffixes);
//! - [`resolve`] — a request's `Host` → the app it serves, through a bounded
//!   60-second cache (negatives included), from **verified** rows only;
//! - [`CustomDomainCtx`] — the request extension the rewrite middleware attaches
//!   on a hit, so the serve path, the sign-in handoff and logout can tell a
//!   custom domain from the product host without parsing the `Host` again.
//!
//! The write side (claim, DNS proof, removal) is `oxy_custom_apps::custom_domains`.
//!
//! ## Why a request extension and not a header
//!
//! A custom domain is not structural — `app.acme.com` names nothing — so unlike
//! the platform subdomain it cannot be re-derived from the `Host` by a pure
//! function wherever it is needed. The middleware resolves it once and hands the
//! answer down in-process. A header would be forgeable by the client.
//!
//! ## Production only
//!
//! A custom domain serves the app's production environment. Staging and
//! sandboxes stay on their platform hosts, where the environment is part of the
//! host label; nothing here parses an environment, so
//! `custom_app_env_request` reads a custom domain as production, which it is.

use std::sync::OnceLock;
use std::time::Duration;

use entity::prelude::{AppCustomDomains, Apps, Organizations};
use entity::{app_custom_domains, apps};
use oxy::database::client::establish_connection;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use uuid::Uuid;

use crate::ttl_lru::TtlLru;

/// A request that arrived on a verified custom domain. Inserted into the
/// request extensions by
/// [`subdomain_rewrite_middleware`](crate::custom_apps_host_dispatch::subdomain_rewrite_middleware).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CustomDomainCtx {
    /// The normalized hostname the request arrived on.
    pub domain: String,
    pub app_id: Uuid,
    pub org_id: Uuid,
    /// The workspace the app is published from.
    pub project_id: Uuid,
    pub org_slug: String,
    pub app_slug: String,
}

/// The TXT record name a domain's ownership is proved at.
pub const CHALLENGE_LABEL: &str = "_oxygen-challenge";
/// What the TXT record's value starts with; the row's token follows.
pub const CHALLENGE_PREFIX: &str = "oxygen-verification=";

/// `_oxygen-challenge.<domain>`.
pub fn challenge_name(domain: &str) -> String {
    format!("{CHALLENGE_LABEL}.{domain}")
}

/// `oxygen-verification=<token>`.
pub fn challenge_value(token: &str) -> String {
    format!("{CHALLENGE_PREFIX}{token}")
}

/// The hostname a customer points their domain's CNAME at, from
/// `OXY_CUSTOM_DOMAIN_TARGET` — the TLS edge that issues certificates on
/// demand (it asks `GET /api/custom-domains/tls-ask` first). `None` on a
/// deployment that has no such edge; the console then says so.
pub fn cname_target() -> Option<String> {
    crate::config::BootConfig::current()
        .hosts
        .custom_domain_target
        .clone()
}

// ── Normalization ─────────────────────────────────────────────────────────

/// Why a hostname cannot be a custom domain. The message is the console's.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("Enter a hostname like app.example.com, without https:// or a path")]
    NotAHostname,
    #[error("An IP address cannot be a custom domain; use a hostname")]
    IpAddress,
    #[error("A custom domain needs at least two labels, like app.example.com")]
    SingleLabel,
    #[error("{0} is reserved and cannot be a custom domain")]
    Reserved(String),
    #[error("{0} is an Oxygen hostname; custom domains must be your own")]
    PlatformZone(String),
}

/// Suffixes that never resolve publicly (RFC 2606 / 6761 / 6762) or name a
/// cluster-internal service. A request whose `Host` ends in one is never looked
/// up, so a probe through the service DNS costs nothing.
const RESERVED_SUFFIXES: &[&str] = &[
    "localhost",
    "local",
    "internal",
    "test",
    "example",
    "invalid",
    "arpa",
];

/// Oxy's own apexes, refused whatever the deployment's environment says: a
/// custom domain under one would let a tenant shadow a platform host.
const PLATFORM_APEXES: &[&str] = &["oxygen-hq.com", "oxy.tech"];

/// The one spelling of a hostname a customer typed: lowercase, punycode for an
/// internationalised name, no trailing dot. Refuses anything that is not a
/// plain public hostname, and anything inside one of this deployment's own
/// zones.
pub fn normalize_domain(input: &str) -> Result<String, DomainError> {
    normalize_domain_against(input, platform_zones())
}

/// [`normalize_domain`] against an explicit zone list — the pure core the
/// tests drive without the environment.
pub fn normalize_domain_against(input: &str, zones: &[String]) -> Result<String, DomainError> {
    let trimmed = input.trim().trim_end_matches('.');
    if trimmed.is_empty()
        || trimmed
            .chars()
            .any(|c| matches!(c, '/' | ':' | '@' | '*' | '?' | '#' | ' ' | '\\'))
    {
        return Err(DomainError::NotAHostname);
    }
    let host = match url::Host::parse(trimmed) {
        Ok(url::Host::Domain(d)) => d.to_ascii_lowercase(),
        Ok(url::Host::Ipv4(_) | url::Host::Ipv6(_)) => return Err(DomainError::IpAddress),
        Err(_) => return Err(DomainError::NotAHostname),
    };
    if host.len() > 253 || !host.split('.').all(valid_label) {
        return Err(DomainError::NotAHostname);
    }
    let Some((_, tld)) = host.rsplit_once('.') else {
        return Err(DomainError::SingleLabel);
    };
    // An all-numeric last label is an IPv4 literal `url` did not read as one
    // (`1.2.3`), never a real TLD.
    if tld.bytes().all(|b| b.is_ascii_digit()) {
        return Err(DomainError::IpAddress);
    }
    if let Some(suffix) = RESERVED_SUFFIXES.iter().find(|s| in_zone(&host, s)) {
        return Err(DomainError::Reserved((*suffix).to_string()));
    }
    if let Some(zone) = zones.iter().find(|z| in_zone(&host, z)) {
        return Err(DomainError::PlatformZone(zone.clone()));
    }
    Ok(host)
}

/// A DNS label as a hostname allows it: 1–63 of `[a-z0-9-]`, not starting or
/// ending with a hyphen. Punycode (`xn--…`) passes.
fn valid_label(label: &str) -> bool {
    !label.is_empty()
        && label.len() <= 63
        && !label.starts_with('-')
        && !label.ends_with('-')
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// `host` is `zone` or a name below it.
fn in_zone(host: &str, zone: &str) -> bool {
    host == zone
        || host
            .strip_suffix(zone)
            .is_some_and(|rest| rest.ends_with('.'))
}

/// Every zone this deployment answers for itself: the platform apexes, the
/// session-cookie zone, the org-subdomain zone, the custom-apps zone and the
/// admin host's parent. Read once — the environment does not change under a
/// running process.
fn platform_zones() -> &'static [String] {
    static ZONES: OnceLock<Vec<String>> = OnceLock::new();
    ZONES.get_or_init(|| {
        let mut zones: Vec<String> = PLATFORM_APEXES.iter().map(|z| z.to_string()).collect();
        let mut push = |z: Option<String>| {
            if let Some(z) = z {
                let z = z.trim().trim_matches('.').to_ascii_lowercase();
                if z.contains('.') && !zones.contains(&z) {
                    zones.push(z);
                }
            }
        };
        push(
            crate::config::BootConfig::current()
                .hosts
                .session_cookie_domain
                .clone(),
        );
        push(crate::org_host_dispatch::org_subdomain_zone());
        push(crate::custom_apps_host_dispatch::custom_apps_zone());
        if let Some(admin) = crate::custom_apps_host_dispatch::admin_base_url()
            .and_then(|u| url::Url::parse(&u).ok())
            .and_then(|u| u.host_str().map(str::to_string))
        {
            push(admin.split_once('.').map(|(_, parent)| parent.to_string()));
            push(Some(admin));
        }
        zones
    })
}

// ── Request-side candidate check ─────────────────────────────────────────

/// The hostname a request's `Host` names, when it could be a custom domain:
/// a dotted, non-IP name outside every platform zone and reserved suffix.
/// `None` is the fast path every product-host request takes — no lookup.
pub fn candidate_host(host: &str) -> Option<String> {
    candidate_host_against(host, platform_zones())
}

/// [`candidate_host`] against an explicit zone list.
pub fn candidate_host_against(host: &str, zones: &[String]) -> Option<String> {
    // An IPv6 literal is bracketed; it is never a custom domain.
    if host.starts_with('[') {
        return None;
    }
    let name = host.split(':').next().unwrap_or(host);
    let name = name.trim_end_matches('.');
    if !name.contains('.') {
        return None;
    }
    let name = name.to_ascii_lowercase();
    if name.parse::<std::net::Ipv4Addr>().is_ok()
        || RESERVED_SUFFIXES.iter().any(|s| in_zone(&name, s))
        || zones.iter().any(|z| in_zone(&name, z))
    {
        return None;
    }
    Some(name)
}

// ── Resolver ──────────────────────────────────────────────────────────────

const CACHE_TTL: Duration = Duration::from_secs(60);
/// Hard bound on cached hostnames, misses included: a spray of forged `Host`
/// headers evicts its own cold entries, not a live domain's.
const CACHE_MAX: usize = 4096;

fn cache() -> &'static TtlLru<String, Option<CustomDomainCtx>> {
    static C: OnceLock<TtlLru<String, Option<CustomDomainCtx>>> = OnceLock::new();
    C.get_or_init(|| TtlLru::new(CACHE_MAX, CACHE_TTL))
}

/// Drop this process's cached answer for `domain`. Called on every write to a
/// domain row; other replicas converge within [`CACHE_TTL`], as the
/// org-subdomain resolver does.
pub fn invalidate(domain: &str) {
    cache().remove(&domain.to_string());
}

/// Drop every cached answer in this process: an app's slug or org changed, so
/// any domain may now name a stale `(org, slug)`.
pub fn invalidate_all() {
    cache().clear();
}

/// The app a verified custom domain serves, for an already-normalized
/// [`candidate_host`]. A transient database failure is not cached: that
/// request is treated as an unknown host and the next one asks again.
pub async fn resolve(domain: &str) -> Option<CustomDomainCtx> {
    let key = domain.to_string();
    if let Some(hit) = cache().get(&key) {
        return hit;
    }
    resolve_fresh(domain).await
}

/// [`resolve`] from the database, never this replica's cache, refreshing the
/// cached answer on the way. For the sign-in handoff, which mints a token on
/// the workspace this names: another replica's move invalidates only its own
/// cache, and a token minted from a stale answer would carry a grant on the
/// workspace the app has left for its whole twelve hours.
pub async fn resolve_fresh(domain: &str) -> Option<CustomDomainCtx> {
    let key = domain.to_string();
    match resolve_from_db(domain).await {
        Ok(resolved) => {
            cache().insert(key, resolved.clone());
            resolved
        }
        Err(()) => None,
    }
}

/// `Ok(None)` is definitive (no verified row, or its app or org is gone).
async fn resolve_from_db(domain: &str) -> Result<Option<CustomDomainCtx>, ()> {
    let db = establish_connection().await.map_err(|e| {
        tracing::warn!("custom domain resolve: db connect failed: {e}");
    })?;
    let row = AppCustomDomains::find()
        .filter(app_custom_domains::Column::Domain.eq(domain))
        .filter(app_custom_domains::Column::VerifiedAt.is_not_null())
        .one(&db)
        .await
        .map_err(|e| tracing::warn!("custom domain resolve: lookup failed: {e}"))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let app = Apps::find()
        .filter(apps::Column::Id.eq(row.app_id))
        .one(&db)
        .await
        .map_err(|e| tracing::warn!("custom domain resolve: app lookup failed: {e}"))?;
    let Some(app) = app else {
        return Ok(None);
    };
    let org = Organizations::find_by_id(app.org_id)
        .one(&db)
        .await
        .map_err(|e| tracing::warn!("custom domain resolve: org lookup failed: {e}"))?;
    Ok(org.map(|org| CustomDomainCtx {
        domain: row.domain,
        app_id: app.id,
        org_id: org.id,
        project_id: app.project_id,
        org_slug: org.slug,
        app_slug: app.slug,
    }))
}

#[cfg(test)]
#[path = "custom_domain_host_tests.rs"]
mod tests;
