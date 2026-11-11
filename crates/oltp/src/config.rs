//! Env-driven OLTP runtime config.
//!
//! Mirrors `airhouse::config`'s tri-state (Enabled / Disabled / Misconfigured):
//! a *partial* configuration must fail startup loudly rather than silently
//! half-working. The distinction that matters operationally is Disabled (nobody
//! asked for this) versus Misconfigured (somebody asked and got it wrong).

use std::sync::OnceLock;

// ── Env var names ─────────────────────────────────────────────────────────────

/// `mock` or `neon`. Unset → the whole integration is off.
pub const OLTP_PROVIDER_VAR: &str = "OXY_OLTP_PROVIDER";
/// Superuser DSN for [`ProviderKind::Local`]; tenant databases are created here.
pub const OLTP_ADMIN_URL_VAR: &str = "OXY_OLTP_ADMIN_URL";
/// Neon credentials. Prefixed like everything else this feature reads: bare
/// `NEON_API_KEY` / `NEON_ORG_ID` looked like machine-wide Neon config rather
/// than one Oxy subsystem's, and did not turn up in a `OXY_OLTP` grep.
pub const NEON_API_KEY_VAR: &str = "OXY_OLTP_NEON_API_KEY";
pub const NEON_ORG_ID_VAR: &str = "OXY_OLTP_NEON_ORG_ID";
/// Provider region id, e.g. `aws-us-east-2`.
pub const OLTP_REGION_VAR: &str = "OXY_OLTP_REGION";
/// Major Postgres version for newly provisioned projects.
pub const OLTP_PG_VERSION_VAR: &str = "OXY_OLTP_PG_VERSION";

const DEFAULT_REGION: &str = "aws-us-east-2";
/// Matches the `postgres:18-alpine` the rest of the repo runs — docker-compose,
/// `oxy start`, CI's services — so a tenant is not a major version behind the
/// cluster every other Oxy component is tested against. Verified against the
/// live Neon API, which accepts 18 for project creation.
pub const DEFAULT_PG_VERSION: u8 = 18;

/// Which backend provisions the per-org Postgres.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderKind {
    /// In-memory fake shaped like the Neon REST API. Provisions nothing real.
    Mock,
    /// Neon REST API.
    Neon { api_key: String, org_id: String },
    /// A Postgres cluster Oxy already has superuser access to — the local
    /// Docker one, in practice.
    ///
    /// Selectable by name rather than inferred from the absence of a provider.
    /// `oxy` loads `.env` itself (`main.rs`), so a shell that unset
    /// `OXY_OLTP_PROVIDER` to mean "local" had it silently restored inside the
    /// binary, and a demo asked to run locally provisioned against Neon.
    Local { admin_url: String },
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::Mock => "mock",
            ProviderKind::Neon { .. } => "neon",
            ProviderKind::Local { .. } => "local",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OltpRuntimeConfig {
    pub provider: ProviderKind,
    pub region: String,
    pub pg_version: u8,
}

/// Three-state config for the per-org OLTP integration.
///
/// - `Enabled` — provider selected and its required vars are present.
/// - `Disabled` — [`OLTP_PROVIDER_VAR`] is unset; the integration is off.
/// - `Misconfigured` — a provider was named but its required vars are missing,
///   or the name isn't recognised. Callers surface this as a startup error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OltpConfig {
    Enabled(OltpRuntimeConfig),
    Disabled,
    Misconfigured(MisconfigReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MisconfigReason {
    /// `OXY_OLTP_PROVIDER` held something other than `mock` / `neon`.
    UnknownProvider(String),
    /// Provider named, but these required vars were missing or empty.
    MissingVars(Vec<&'static str>),
}

impl std::fmt::Display for MisconfigReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MisconfigReason::UnknownProvider(v) => write!(
                f,
                "{OLTP_PROVIDER_VAR}={v:?} is not a known provider (expected `local`, `mock` or `neon`)"
            ),
            MisconfigReason::MissingVars(vars) => write!(
                f,
                "OLTP integration is partially configured — also set: {}",
                vars.join(", ")
            ),
        }
    }
}

static CACHED_CONFIG: OnceLock<OltpConfig> = OnceLock::new();

impl OltpConfig {
    /// Load from environment. Re-reads env vars every call; safe for tests.
    pub fn from_env() -> Self {
        Self::from_lookup(|k| std::env::var(k).ok())
    }

    /// Cached form of [`Self::from_env`] — reads env vars once on first call.
    ///
    /// Restart the process after changing any `OXY_OLTP_*` / `NEON_*` var.
    pub fn cached() -> &'static OltpConfig {
        CACHED_CONFIG.get_or_init(Self::from_env)
    }

    /// Testable core: resolves against an arbitrary lookup rather than the
    /// process environment, so the tri-state can be exercised without the
    /// global-env races that make `std::env::set_var` unsound under nextest.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let get = |k: &str| lookup(k).filter(|s| !s.trim().is_empty());

        let Some(provider_name) = get(OLTP_PROVIDER_VAR) else {
            return Self::Disabled;
        };

        let provider = match provider_name.trim().to_ascii_lowercase().as_str() {
            "mock" => ProviderKind::Mock,
            // No OXY_DATABASE_URL fallback: that is Oxy's own control plane,
            // and silently provisioning tenant databases (and running CREATE
            // ROLE) there is not something a missing variable should decide. The
            // admin URL must be given EXPLICITLY. `oxy start` (dev box) supplies
            // it — pointed at the throwaway Docker cluster it just started, not
            // the control plane (see `oxy-server`'s `commands::start`) — but this parser
            // never infers it, so a production `serve` with `local` and no admin
            // URL stays `Misconfigured` rather than reaching for the control plane.
            "local" => match get(OLTP_ADMIN_URL_VAR) {
                Some(admin_url) => ProviderKind::Local { admin_url },
                None => {
                    return Self::Misconfigured(MisconfigReason::MissingVars(vec![
                        OLTP_ADMIN_URL_VAR,
                    ]));
                }
            },
            "neon" => {
                let api_key = get(NEON_API_KEY_VAR);
                let org_id = get(NEON_ORG_ID_VAR);
                let mut missing = Vec::new();
                if api_key.is_none() {
                    missing.push(NEON_API_KEY_VAR);
                }
                if org_id.is_none() {
                    missing.push(NEON_ORG_ID_VAR);
                }
                match (api_key, org_id) {
                    (Some(api_key), Some(org_id)) => ProviderKind::Neon { api_key, org_id },
                    _ => return Self::Misconfigured(MisconfigReason::MissingVars(missing)),
                }
            }
            other => {
                return Self::Misconfigured(MisconfigReason::UnknownProvider(other.to_string()));
            }
        };

        let region = get(OLTP_REGION_VAR).unwrap_or_else(|| DEFAULT_REGION.to_string());
        let pg_version = get(OLTP_PG_VERSION_VAR)
            .map(|s| {
                s.parse::<u8>().unwrap_or_else(|_| {
                    tracing::warn!(
                        "{OLTP_PG_VERSION_VAR} value {s:?} is not a valid major version; \
                         falling back to {DEFAULT_PG_VERSION}"
                    );
                    DEFAULT_PG_VERSION
                })
            })
            .unwrap_or(DEFAULT_PG_VERSION);

        Self::Enabled(OltpRuntimeConfig {
            provider,
            region,
            pg_version,
        })
    }

    pub fn as_runtime(&self) -> Option<&OltpRuntimeConfig> {
        match self {
            Self::Enabled(c) => Some(c),
            _ => None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self, Self::Enabled(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The literal names, pinned.
    ///
    /// Every other test here goes through the `*_VAR` constants, so renaming
    /// one keeps them all green while the `.env` on a deployment stops
    /// matching — and the failure is `Disabled`, i.e. the feature quietly
    /// turns off rather than erroring. These are a contract with the outside
    /// world; changing one is a deployment change, not a refactor.
    ///
    /// One prefix, deliberately: `NEON_API_KEY` and `NEON_ORG_ID` read as
    /// machine-wide Neon config rather than one Oxy subsystem's, and did not
    /// appear in a grep for `OXY_OLTP`.
    #[test]
    fn env_var_names_are_the_ones_documented() {
        assert_eq!(OLTP_PROVIDER_VAR, "OXY_OLTP_PROVIDER");
        assert_eq!(OLTP_ADMIN_URL_VAR, "OXY_OLTP_ADMIN_URL");
        assert_eq!(NEON_API_KEY_VAR, "OXY_OLTP_NEON_API_KEY");
        assert_eq!(NEON_ORG_ID_VAR, "OXY_OLTP_NEON_ORG_ID");
        assert_eq!(OLTP_REGION_VAR, "OXY_OLTP_REGION");
        assert_eq!(OLTP_PG_VERSION_VAR, "OXY_OLTP_PG_VERSION");

        for name in [
            OLTP_PROVIDER_VAR,
            OLTP_ADMIN_URL_VAR,
            NEON_API_KEY_VAR,
            NEON_ORG_ID_VAR,
            OLTP_REGION_VAR,
            OLTP_PG_VERSION_VAR,
        ] {
            assert!(
                name.starts_with("OXY_OLTP_"),
                "{name} is outside the namespace"
            );
        }
    }

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        move |k: &str| {
            owned
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn no_provider_var_is_disabled() {
        assert_eq!(OltpConfig::from_lookup(lookup(&[])), OltpConfig::Disabled);
    }

    #[test]
    fn empty_provider_var_is_disabled_not_misconfigured() {
        // An empty string is how a var reads when it's declared-but-unset in a
        // k8s manifest; treat it as "off", not as a broken config.
        let cfg = OltpConfig::from_lookup(lookup(&[(OLTP_PROVIDER_VAR, "   ")]));
        assert_eq!(cfg, OltpConfig::Disabled);
    }

    #[test]
    fn mock_provider_needs_nothing_else() {
        let cfg = OltpConfig::from_lookup(lookup(&[(OLTP_PROVIDER_VAR, "mock")]));
        let rt = cfg.as_runtime().expect("enabled");
        assert_eq!(rt.provider, ProviderKind::Mock);
        assert_eq!(rt.region, DEFAULT_REGION);
        assert_eq!(rt.pg_version, DEFAULT_PG_VERSION);
    }

    #[test]
    fn provider_name_is_case_insensitive() {
        let cfg = OltpConfig::from_lookup(lookup(&[(OLTP_PROVIDER_VAR, "MOCK")]));
        assert!(cfg.is_enabled());
    }

    #[test]
    fn neon_without_credentials_is_misconfigured_and_names_both_vars() {
        let cfg = OltpConfig::from_lookup(lookup(&[(OLTP_PROVIDER_VAR, "neon")]));
        assert_eq!(
            cfg,
            OltpConfig::Misconfigured(MisconfigReason::MissingVars(vec![
                NEON_API_KEY_VAR,
                NEON_ORG_ID_VAR
            ]))
        );
    }

    #[test]
    fn neon_with_partial_credentials_names_only_the_missing_one() {
        let cfg = OltpConfig::from_lookup(lookup(&[
            (OLTP_PROVIDER_VAR, "neon"),
            (NEON_API_KEY_VAR, "key"),
        ]));
        assert_eq!(
            cfg,
            OltpConfig::Misconfigured(MisconfigReason::MissingVars(vec![NEON_ORG_ID_VAR]))
        );
    }

    #[test]
    fn neon_fully_configured_is_enabled() {
        let cfg = OltpConfig::from_lookup(lookup(&[
            (OLTP_PROVIDER_VAR, "neon"),
            (NEON_API_KEY_VAR, "key"),
            (NEON_ORG_ID_VAR, "org"),
            (OLTP_REGION_VAR, "aws-eu-central-1"),
        ]));
        let rt = cfg.as_runtime().expect("enabled");
        assert_eq!(
            rt.provider,
            ProviderKind::Neon {
                api_key: "key".into(),
                org_id: "org".into()
            }
        );
        assert_eq!(rt.region, "aws-eu-central-1");
    }

    #[test]
    fn unknown_provider_is_misconfigured() {
        let cfg = OltpConfig::from_lookup(lookup(&[(OLTP_PROVIDER_VAR, "supabase")]));
        assert_eq!(
            cfg,
            OltpConfig::Misconfigured(MisconfigReason::UnknownProvider("supabase".into()))
        );
    }

    #[test]
    fn garbage_pg_version_falls_back_rather_than_failing_startup() {
        let cfg = OltpConfig::from_lookup(lookup(&[
            (OLTP_PROVIDER_VAR, "mock"),
            (OLTP_PG_VERSION_VAR, "seventeen"),
        ]));
        assert_eq!(cfg.as_runtime().unwrap().pg_version, DEFAULT_PG_VERSION);
    }
}
