use oxy::constants::{
    OXY_SLACK_APP_BASE_URL_VAR, OXY_SLACK_APP_LEVEL_TOKEN_VAR, OXY_SLACK_CHART_UPLOAD_VAR,
    OXY_SLACK_CLIENT_ID_VAR, OXY_SLACK_CLIENT_SECRET_VAR, OXY_SLACK_ENABLED_VAR,
    OXY_SLACK_SIGNING_SECRET_VAR,
};
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackRuntimeConfig {
    pub client_id: String,
    pub client_secret: String,
    pub signing_secret: String,
    pub app_base_url: String,
    pub app_level_token: Option<String>,
}

/// Single-enum replacement for the former `(Option<SlackRuntimeConfig>, SlackStatus)` tuple.
/// Callers that only need the config can call `into_runtime()`; callers that need to
/// distinguish Disabled from Misconfigured should match on `Self` directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlackConfig {
    /// Fully configured and active.
    Enabled(SlackRuntimeConfig),
    /// Explicitly disabled via OXY_SLACK_ENABLED=false.
    Disabled,
    /// One or more required env vars missing; webhooks will 503.
    Misconfigured,
}

static CACHED_CONFIG: OnceLock<SlackConfig> = OnceLock::new();

impl SlackConfig {
    /// Load from environment.
    ///
    /// Reads env vars every time — suitable for tests that modify the
    /// environment. For hot paths (webhook handlers) prefer `cached()`.
    pub fn from_env() -> Self {
        let enabled = std::env::var(OXY_SLACK_ENABLED_VAR)
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);
        if !enabled {
            return Self::Disabled;
        }

        let client_id = std::env::var(OXY_SLACK_CLIENT_ID_VAR).ok();
        let client_secret = std::env::var(OXY_SLACK_CLIENT_SECRET_VAR).ok();
        let signing_secret = std::env::var(OXY_SLACK_SIGNING_SECRET_VAR).ok();
        let app_base_url = std::env::var(OXY_SLACK_APP_BASE_URL_VAR).ok();

        match (client_id, client_secret, signing_secret, app_base_url) {
            (Some(ci), Some(cs), Some(ss), Some(base))
                if !ci.is_empty() && !cs.is_empty() && !ss.is_empty() && !base.is_empty() =>
            {
                let app_base_url = base.trim_end_matches('/').to_string();
                let app_level_token = std::env::var(OXY_SLACK_APP_LEVEL_TOKEN_VAR)
                    .ok()
                    .filter(|v| !v.is_empty());
                Self::Enabled(SlackRuntimeConfig {
                    client_id: ci,
                    client_secret: cs,
                    signing_secret: ss,
                    app_base_url,
                    app_level_token,
                })
            }
            _ => Self::Misconfigured,
        }
    }

    /// Cached form of `from_env` — reads env vars once on the first call.
    ///
    /// Use this in hot paths (webhook handlers). Tests should use `from_env()`
    /// which re-reads every time, ensuring env-var overrides take effect.
    pub fn cached() -> &'static SlackConfig {
        CACHED_CONFIG.get_or_init(Self::from_env)
    }

    /// Returns a reference to the runtime config if fully enabled.
    /// Use with `cached()` to avoid cloning.
    pub fn as_runtime(&self) -> Option<&SlackRuntimeConfig> {
        match self {
            Self::Enabled(c) => Some(c),
            _ => None,
        }
    }

    /// Returns the runtime config if fully enabled. Returns `None` for Disabled
    /// or Misconfigured. Call sites that need the specific reason should match
    /// on `Self` directly.
    pub fn into_runtime(self) -> Option<SlackRuntimeConfig> {
        match self {
            Self::Enabled(c) => Some(c),
            _ => None,
        }
    }
}

/// Whether chart PNGs are uploaded to Slack via `files.uploadV2`.
///
/// Read from `OXY_SLACK_CHART_UPLOAD` exactly once on first access and
/// cached for the lifetime of the process — flipping the env var at
/// runtime has no effect, matching how the rest of the Slack config
/// behaves. Set to `1`/`true` in production deploys; defaults off so
/// local dev keeps the on-disk PNG breadcrumb path.
pub fn chart_upload_enabled() -> bool {
    static CACHED: OnceLock<bool> = OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var(OXY_SLACK_CHART_UPLOAD_VAR)
            .ok()
            .as_deref()
            .map(|v| matches!(v.trim(), "1" | "true" | "True" | "TRUE"))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // NOTE: this mutex only serialises tests within this unit-test binary.
    // If these tests are ever moved into a separate integration-test crate,
    // use the `serial_test` crate (or an explicit process-wide lock) instead.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_clean_env<F: FnOnce()>(f: F) {
        let _g = ENV_LOCK.lock().unwrap();
        for k in [
            OXY_SLACK_ENABLED_VAR,
            OXY_SLACK_CLIENT_ID_VAR,
            OXY_SLACK_CLIENT_SECRET_VAR,
            OXY_SLACK_SIGNING_SECRET_VAR,
            OXY_SLACK_APP_BASE_URL_VAR,
            OXY_SLACK_APP_LEVEL_TOKEN_VAR,
        ] {
            unsafe {
                std::env::remove_var(k);
            }
        }
        f();
    }

    #[test]
    fn returns_disabled_when_explicit_false() {
        with_clean_env(|| {
            unsafe {
                std::env::set_var(OXY_SLACK_ENABLED_VAR, "false");
            }
            assert_eq!(SlackConfig::from_env(), SlackConfig::Disabled);
        });
    }

    #[test]
    fn returns_misconfigured_when_any_required_missing() {
        with_clean_env(|| {
            unsafe {
                std::env::set_var(OXY_SLACK_CLIENT_ID_VAR, "id");
                std::env::set_var(OXY_SLACK_CLIENT_SECRET_VAR, "sec");
                // missing signing_secret + base_url
            }
            assert_eq!(SlackConfig::from_env(), SlackConfig::Misconfigured);
        });
    }

    #[test]
    fn returns_enabled_when_all_present() {
        with_clean_env(|| {
            unsafe {
                std::env::set_var(OXY_SLACK_CLIENT_ID_VAR, "ci");
                std::env::set_var(OXY_SLACK_CLIENT_SECRET_VAR, "cs");
                std::env::set_var(OXY_SLACK_SIGNING_SECRET_VAR, "ss");
                std::env::set_var(OXY_SLACK_APP_BASE_URL_VAR, "https://app.oxygen-hq.com/");
            }
            let cfg = SlackConfig::from_env();
            let runtime = cfg.into_runtime().expect("should be Enabled");
            assert_eq!(runtime.app_base_url, "https://app.oxygen-hq.com"); // trailing slash trimmed
            assert_eq!(runtime.app_level_token, None); // no app-level token set
        });
    }

    #[test]
    fn returns_enabled_with_app_level_token_when_set() {
        with_clean_env(|| {
            unsafe {
                std::env::set_var(OXY_SLACK_CLIENT_ID_VAR, "ci");
                std::env::set_var(OXY_SLACK_CLIENT_SECRET_VAR, "cs");
                std::env::set_var(OXY_SLACK_SIGNING_SECRET_VAR, "ss");
                std::env::set_var(OXY_SLACK_APP_BASE_URL_VAR, "https://app.oxygen-hq.com");
                std::env::set_var(OXY_SLACK_APP_LEVEL_TOKEN_VAR, "xapp-1-test-token");
            }
            let cfg = SlackConfig::from_env();
            let runtime = cfg.into_runtime().expect("should be Enabled");
            assert_eq!(
                runtime.app_level_token.as_deref(),
                Some("xapp-1-test-token")
            );
        });
    }
}
