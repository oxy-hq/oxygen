//! The hosts and zones a deployment answers on, from six variables that were
//! read in three files (`org_host_dispatch`, `custom_apps_host_dispatch`,
//! `custom_domain_host`). The derivations are unchanged; they moved here so
//! `OXY_API_URL` is parsed once and a malformed one is a boot error instead of
//! a silently absent zone.

use super::ConfigError;

/// Normalise a zone: trim, drop leading and trailing dots, lower-case; `None`
/// when nothing is left.
fn zone(raw: Option<String>) -> Option<String> {
    let z = raw?;
    let z = z.trim().trim_start_matches('.').trim_end_matches('.');
    (!z.is_empty()).then(|| z.to_ascii_lowercase())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostsConfig {
    /// `OXY_API_URL`, as set: the admin host's public API URL
    /// (`https://app-dev.oxygen-hq.com/api`). Everything below derives from it
    /// when its own variable is unset.
    pub api_url: Option<String>,
    /// `OXY_ORG_SUBDOMAIN_ZONE`, normalised. Required on non-prod, where the apex
    /// wildcard cannot be shared across environments.
    pub explicit_org_zone: Option<String>,
    /// `OXY_CUSTOM_APPS_ZONE`, normalised.
    pub explicit_custom_apps_zone: Option<String>,
    /// `OXY_RESERVED_SUBDOMAINS`: extra labels (trimmed, lower-cased, non-empty)
    /// that may never be an org subdomain, on top of the built-in list.
    pub reserved_subdomains: Vec<String>,
    /// `OXY_CUSTOM_DOMAIN_TARGET`: the CNAME target customers point a custom
    /// domain at (the TLS edge), trimmed, lower-cased, trailing dot dropped.
    pub custom_domain_target: Option<String>,
    /// `OXY_SESSION_COOKIE_DOMAIN`, as set: the parent domain session cookies are
    /// scoped to. Consumers trim it; the auth code reads the variable in more
    /// shapes than one and is not migrated yet.
    pub session_cookie_domain: Option<String>,
}

impl HostsConfig {
    pub fn parse(var: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            api_url: var("OXY_API_URL"),
            explicit_org_zone: zone(var("OXY_ORG_SUBDOMAIN_ZONE")),
            explicit_custom_apps_zone: zone(var("OXY_CUSTOM_APPS_ZONE")),
            reserved_subdomains: var("OXY_RESERVED_SUBDOMAINS")
                .map(|extra| {
                    extra
                        .split(',')
                        .map(|s| s.trim().to_ascii_lowercase())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default(),
            custom_domain_target: var("OXY_CUSTOM_DOMAIN_TARGET").and_then(|raw| {
                let target = raw.trim().trim_end_matches('.').to_ascii_lowercase();
                (!target.is_empty()).then_some(target)
            }),
            session_cookie_domain: var("OXY_SESSION_COOKIE_DOMAIN"),
        }
    }

    /// `OXY_API_URL` must be a URL with a host when it is set at all.
    pub fn validate(&self) -> Result<(), ConfigError> {
        match &self.api_url {
            Some(raw) if self.api_url_parsed().is_none() => Err(ConfigError {
                var: "OXY_API_URL",
                value: raw.clone(),
                problem: "expected an absolute URL with a host, like \
                          https://app-dev.oxygen-hq.com/api"
                    .to_string(),
            }),
            _ => Ok(()),
        }
    }

    /// `OXY_API_URL` parsed, when it is set and parses to a URL with a host.
    pub fn api_url_parsed(&self) -> Option<url::Url> {
        let parsed: url::Url = self.api_url.as_deref()?.parse().ok()?;
        parsed.host_str()?;
        Some(parsed)
    }

    /// The admin host's first label and the rest (`app-dev`, `oxygen-hq.com`).
    fn admin_host_split(&self) -> Option<(String, String)> {
        let parsed = self.api_url_parsed()?;
        let host = parsed.host_str()?;
        let (first, rest) = host.split_once('.')?;
        Some((first.to_string(), rest.to_string()))
    }

    /// The DNS zone bare org subdomains live under.
    ///
    /// The explicit zone wins. Otherwise the admin host's parent, but **only**
    /// when its first label is exactly `app` (prod apex: `app.oxygen-hq.com` →
    /// `oxygen-hq.com`). An `app-<env>` or custom host derives nothing, to keep
    /// the apex wildcard from colliding across environments; those set the
    /// variable.
    pub fn org_subdomain_zone(&self) -> Option<String> {
        if let Some(z) = &self.explicit_org_zone {
            return Some(z.clone());
        }
        let (first, rest) = self.admin_host_split()?;
        (first == "app").then(|| rest.to_ascii_lowercase())
    }

    /// The DNS zone custom-app subdomains live under: the explicit zone, else
    /// `app{-env}.<rest>` → `customer-apps{-env}.<rest>` from the admin host,
    /// else `customer-apps.` under the org zone (dev's `aip.dev.oxy.tech` sets
    /// `OXY_ORG_SUBDOMAIN_ZONE=dev.oxy.tech`, so its apps live under
    /// `customer-apps.dev.oxy.tech`).
    pub fn custom_apps_zone(&self) -> Option<String> {
        if let Some(z) = &self.explicit_custom_apps_zone {
            return Some(z.clone());
        }
        if let Some((first, rest)) = self.admin_host_split()
            && let Some(env_suffix) = first.strip_prefix("app")
        {
            return Some(format!("customer-apps{env_suffix}.{rest}"));
        }
        let org_zone = self.org_subdomain_zone()?;
        Some(format!("customer-apps.{org_zone}"))
    }

    /// Absolute base URL of the admin host, `https://app-staging.oxygen-hq.com`,
    /// with the port when the URL carries one. `None` when `OXY_API_URL` is
    /// unset or malformed.
    pub fn admin_base_url(&self) -> Option<String> {
        let parsed = self.api_url_parsed()?;
        let scheme = parsed.scheme();
        let host = parsed.host_str()?;
        match parsed.port() {
            Some(port) => Some(format!("{scheme}://{host}:{port}")),
            None => Some(format!("{scheme}://{host}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn hosts(pairs: &[(&str, &str)]) -> HostsConfig {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        HostsConfig::parse(&|name| map.get(name).cloned())
    }

    #[test]
    fn zones_are_normalised_and_empty_ones_dropped() {
        let h = hosts(&[
            ("OXY_ORG_SUBDOMAIN_ZONE", ".Dev.Oxygen-HQ.com."),
            ("OXY_CUSTOM_APPS_ZONE", "   "),
            ("OXY_RESERVED_SUBDOMAINS", "pokehouse, Special ,,"),
            ("OXY_CUSTOM_DOMAIN_TARGET", " Edge.Oxygen-HQ.com. "),
        ]);
        assert_eq!(h.explicit_org_zone.as_deref(), Some("dev.oxygen-hq.com"));
        assert_eq!(h.explicit_custom_apps_zone, None);
        assert_eq!(h.reserved_subdomains, vec!["pokehouse", "special"]);
        assert_eq!(
            h.custom_domain_target.as_deref(),
            Some("edge.oxygen-hq.com")
        );
    }

    #[test]
    fn the_org_zone_derives_only_from_a_bare_app_host() {
        let prod = hosts(&[("OXY_API_URL", "https://app.oxygen-hq.com/api")]);
        assert_eq!(prod.org_subdomain_zone().as_deref(), Some("oxygen-hq.com"));
        let dev = hosts(&[("OXY_API_URL", "https://app-dev.oxygen-hq.com/api")]);
        assert_eq!(dev.org_subdomain_zone(), None);
        assert_eq!(
            dev.custom_apps_zone().as_deref(),
            Some("customer-apps-dev.oxygen-hq.com")
        );
        let local = hosts(&[("OXY_API_URL", "http://localhost:3000/api")]);
        assert_eq!(local.org_subdomain_zone(), None);
        assert_eq!(local.custom_apps_zone(), None);
        assert_eq!(
            local.admin_base_url().as_deref(),
            Some("http://localhost:3000")
        );
    }

    #[test]
    fn the_custom_apps_zone_falls_back_to_the_org_zone() {
        let h = hosts(&[
            ("OXY_API_URL", "https://aip.dev.oxy.tech/api"),
            ("OXY_ORG_SUBDOMAIN_ZONE", "dev.oxy.tech"),
        ]);
        assert_eq!(
            h.custom_apps_zone().as_deref(),
            Some("customer-apps.dev.oxy.tech")
        );
    }

    #[test]
    fn a_malformed_api_url_fails_validation_and_derives_nothing() {
        let h = hosts(&[("OXY_API_URL", "app.oxygen-hq.com/api")]);
        assert_eq!(h.validate().unwrap_err().var, "OXY_API_URL");
        assert_eq!(h.admin_base_url(), None);
        assert_eq!(h.org_subdomain_zone(), None);
        assert_eq!(hosts(&[]).validate(), Ok(()));
    }
}
