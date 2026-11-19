//! What `oxy-app`'s Airhouse connection pool reads about its deployment.

/// The pool's process-wide connection ceiling.
///
/// One knob for now. The pool's other settings
/// (`OXY_AIRHOUSE_POOL_CONNS_PER_IDENTITY`, `..._IDLE_SECS`,
/// `..._MAX_IDENTITIES`, `..._MAX_APP_IDENTITIES`,
/// `..._MAX_PREVIEW_IDENTITIES`, `..._DISABLED`) are still read where they are
/// used and sit in the env-read backlog; this is the concern they belong in
/// when that backlog shrinks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AirhousePoolConfig {
    /// `OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS`: how many Airhouse connections one
    /// Oxy process may hold open at once, across the analytics path,
    /// `ctx.airhouse` and workspace previews together.
    ///
    /// This is the only setting that bounds the **total**: the pool's identity
    /// caps are per identity or per identity class, and nothing can see their
    /// product. Airhouse's shared data plane admits a bounded number of
    /// connections per pod across all tenants, so without a ceiling one Oxy
    /// process can exhaust it by itself.
    ///
    /// The default is a conservative backstop, not a sized value: the data
    /// plane's own admission cap and replica count are not configured in this
    /// repository. Size it per deployment — see
    /// `internal-docs/airhouse-connection-scaling.md`.
    pub max_live_conns: usize,
}

/// The default ceiling. See [`AirhousePoolConfig::max_live_conns`] for why this
/// is a backstop rather than a tuned figure.
pub const DEFAULT_MAX_LIVE_CONNS: usize = 24;

impl AirhousePoolConfig {
    pub fn parse(var: &dyn Fn(&str) -> Option<String>) -> Self {
        Self {
            max_live_conns: var("OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS")
                .and_then(|v| v.trim().parse::<usize>().ok())
                // `0` would make every connect wait for a permit that can
                // never exist, so it falls back to the default like any other
                // unusable value rather than wedging the process.
                .filter(|n| *n > 0)
                .unwrap_or(DEFAULT_MAX_LIVE_CONNS),
        }
    }
}

impl Default for AirhousePoolConfig {
    fn default() -> Self {
        Self {
            max_live_conns: DEFAULT_MAX_LIVE_CONNS,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{AirhousePoolConfig, DEFAULT_MAX_LIVE_CONNS};

    fn parse(pairs: &[(&str, &str)]) -> AirhousePoolConfig {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        AirhousePoolConfig::parse(&|name| map.get(name).cloned())
    }

    #[test]
    fn an_unconfigured_deployment_gets_the_default() {
        assert_eq!(parse(&[]).max_live_conns, DEFAULT_MAX_LIVE_CONNS);
    }

    #[test]
    fn a_configured_ceiling_is_taken_as_given() {
        assert_eq!(
            parse(&[("OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS", "64")]).max_live_conns,
            64
        );
        // Whitespace from a YAML value should not cost the deployment its setting.
        assert_eq!(
            parse(&[("OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS", " 8 ")]).max_live_conns,
            8
        );
    }

    #[test]
    fn a_zero_ceiling_falls_back_instead_of_wedging_the_process() {
        // A ceiling of nothing would make every connect wait for a permit that
        // cannot exist. Taking it literally would stop the process talking to
        // Airhouse at all, which no deployment can have meant.
        assert_eq!(
            parse(&[("OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS", "0")]).max_live_conns,
            DEFAULT_MAX_LIVE_CONNS
        );
    }

    #[test]
    fn an_unusable_value_falls_back_rather_than_refusing_the_boot() {
        for bad in ["", "lots", "-4", "3.5"] {
            assert_eq!(
                parse(&[("OXY_AIRHOUSE_POOL_MAX_LIVE_CONNS", bad)]).max_live_conns,
                DEFAULT_MAX_LIVE_CONNS,
                "{bad:?} should fall back"
            );
        }
    }
}
