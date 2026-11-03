//! The `fetch` plane's setting, and the registry of third-party sandboxes an
//! `allow` environment may send writes to
//! (`internal-docs/2026-11-02-custom-app-branches-design.md` §3 row 10, §5).
//!
//! `held-writes` is the default and holds every non-read call. `allow` names
//! the provider sandbox hosts a call may really reach, and nothing else: the
//! list is checked against [`PROVIDER_SANDBOXES`] here and again when a call
//! is made, so a host dropped from the registry stops being sent to.

use serde::Deserialize;

use super::FetchMode;

/// The most hosts one `fetch: allow` setting may name. A backstop for a future,
/// larger registry: duplicates are refused and every host must be registered,
/// so a list longer than [`PROVIDER_SANDBOXES`] fails those checks first and
/// this cannot trigger while the registry is smaller.
const MAX_HOSTS: usize = 16;

/// `planes.fetch` as written: the mode alone, or the mode with the hosts it
/// needs.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(try_from = "serde_json::Value")]
pub enum FetchSetting {
    /// `"held-writes"` (or `"allow"`, which [`validate_fetch`] refuses: it has
    /// no hosts).
    Mode(FetchMode),
    /// `{"mode": "allow", "hosts": ["sandbox-quickbooks.api.intuit.com"]}`.
    Spec { mode: FetchMode, hosts: Vec<String> },
}

/// The object form, its own struct so `deny_unknown_fields` applies to it: a
/// typo'd key is an error, never ignored.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFetchSpec {
    mode: FetchMode,
    #[serde(default)]
    hosts: Vec<String>,
}

/// Read by hand rather than as an untagged enum, which would answer any bad
/// object with "did not match any variant" and never name the key.
impl TryFrom<serde_json::Value> for FetchSetting {
    type Error = String;

    fn try_from(value: serde_json::Value) -> Result<Self, String> {
        if value.is_object() {
            let RawFetchSpec { mode, hosts } =
                serde_json::from_value(value).map_err(|e| e.to_string())?;
            return Ok(FetchSetting::Spec { mode, hosts });
        }
        serde_json::from_value(value)
            .map(FetchSetting::Mode)
            .map_err(|e| e.to_string())
    }
}

impl FetchSetting {
    /// The mode, whichever form the setting was written in.
    pub fn mode(&self) -> FetchMode {
        match self {
            FetchSetting::Mode(mode) | FetchSetting::Spec { mode, .. } => *mode,
        }
    }

    /// The hosts as written (not lowercased); empty for the bare-mode form.
    pub fn hosts(&self) -> &[String] {
        match self {
            FetchSetting::Mode(_) => &[],
            FetchSetting::Spec { hosts, .. } => hosts,
        }
    }
}

/// What a call to a registered sandbox host must show before it is sent. The
/// host alone is not always enough: a provider can serve a sandbox and live
/// traffic from one host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxAdmits {
    /// The host is sandbox-only, so any call to it is admitted.
    Host,
    /// The request's key must be a Stripe test key (`sk_test_` or `rk_test_`).
    StripeTestKey,
    /// The path must name a QuickBooks sandbox company registered for previews.
    QuickBooksCompany,
}

/// One provider sandbox a `fetch: allow` environment may send to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderSandbox {
    /// The lowercase hostname a call goes to.
    pub host: &'static str,
    /// The provider's name, for messages.
    pub provider: &'static str,
    /// What a call to the host must show to be sent.
    pub admits: SandboxAdmits,
}

/// Every host `fetch: allow` can name. Adding a host is a code change: a
/// customer cannot make a live endpoint sendable by writing it in a setting.
pub const PROVIDER_SANDBOXES: &[ProviderSandbox] = &[
    // Shared with the live API's data model but a separate host; only
    // companies registered as preview sources are admitted.
    ProviderSandbox {
        host: "sandbox-quickbooks.api.intuit.com",
        provider: "QuickBooks",
        admits: SandboxAdmits::QuickBooksCompany,
    },
    // One host for test and live mode; the key's prefix tells them apart.
    ProviderSandbox {
        host: "api.stripe.com",
        provider: "Stripe",
        admits: SandboxAdmits::StripeTestKey,
    },
    // Square's sandbox host serves no production traffic.
    ProviderSandbox {
        host: "connect.squareupsandbox.com",
        provider: "Square",
        admits: SandboxAdmits::Host,
    },
    // PayPal's sandbox host serves no production traffic.
    ProviderSandbox {
        host: "api-m.sandbox.paypal.com",
        provider: "PayPal",
        admits: SandboxAdmits::Host,
    },
    // Plaid's sandbox host serves no production traffic.
    ProviderSandbox {
        host: "sandbox.plaid.com",
        provider: "Plaid",
        admits: SandboxAdmits::Host,
    },
    // Toast's sandbox host serves no production traffic.
    ProviderSandbox {
        host: "ws-sandbox-api.eng.toasttab.com",
        provider: "Toast",
        admits: SandboxAdmits::Host,
    },
];

/// The registered sandbox for `host`, compared case-insensitively. `None` for
/// anything not in [`PROVIDER_SANDBOXES`], including a host with a scheme,
/// port or path.
pub fn provider_sandbox(host: &str) -> Option<&'static ProviderSandbox> {
    PROVIDER_SANDBOXES
        .iter()
        .find(|sandbox| sandbox.host.eq_ignore_ascii_case(host))
}

/// Check a `fetch` setting an environment may be saved with. `allow` needs at
/// least one host, `held-writes` takes none, and every host must be a bare
/// registered sandbox hostname, at most [`MAX_HOSTS`], none repeated.
pub fn validate_fetch(setting: &FetchSetting) -> Result<(), String> {
    let hosts = setting.hosts();
    match setting.mode() {
        FetchMode::HeldWrites if !hosts.is_empty() => {
            return Err("fetch: held-writes takes no hosts".to_string());
        }
        FetchMode::HeldWrites => return Ok(()),
        FetchMode::Allow if hosts.is_empty() => {
            return Err("fetch: allow needs at least one host".to_string());
        }
        FetchMode::Allow => {}
    }
    if hosts.len() > MAX_HOSTS {
        return Err(format!(
            "fetch: at most {MAX_HOSTS} hosts, got {}",
            hosts.len()
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for host in hosts {
        validate_host(host)?;
        if !seen.insert(host.to_ascii_lowercase()) {
            return Err(format!("fetch: host {host} is listed twice"));
        }
    }
    Ok(())
}

/// One host: a bare hostname (no scheme, port, path or userinfo) that is a
/// registered sandbox.
fn validate_host(host: &str) -> Result<(), String> {
    let not_bare = host.is_empty()
        || host.contains("://")
        || host
            .chars()
            .any(|c| matches!(c, ':' | '/' | '@') || c.is_whitespace());
    if not_bare {
        return Err(format!(
            "fetch: host {host:?} must be a bare hostname, with no scheme, port, path or userinfo"
        ));
    }
    if provider_sandbox(host).is_none() {
        let known: Vec<&str> = PROVIDER_SANDBOXES.iter().map(|s| s.host).collect();
        return Err(format!(
            "fetch: {host} is not a registered provider sandbox (known: {})",
            known.join(", ")
        ));
    }
    Ok(())
}
