//! How a deployment starts the promote checks' runner
//! (`oxy_api_custom_apps::custom_apps_build_checks::dispatch`): the GitHub
//! Actions workflow it dispatches for every build that reaches a custom app's
//! staging, and which deployment it tells that workflow to check.
//!
//! * `OXY_APP_CHECKS_GITHUB_TOKEN`: a GitHub token that may dispatch the
//!   workflow (`actions: write` on the repository). A secret: [`Debug`] never
//!   prints it, and nothing here logs it.
//! * `OXY_APP_CHECKS_GITHUB_REPO`: `owner/name` of the repository that holds
//!   the workflow. Default `oxy-hq/oxygen-internal`.
//! * `OXY_APP_CHECKS_GITHUB_WORKFLOW`: the workflow's file name. Default
//!   `custom-app-build-checks.yaml`.
//! * `OXY_APP_CHECKS_GITHUB_REF`: the branch or tag the workflow runs from.
//!   Default `main`.
//! * `OXY_APP_CHECKS_DEPLOYMENT`: which deployment this is, as the workflow
//!   names them: `dev`, `staging` or `prod`. The workflow picks the URL and
//!   the checker's token from it.
//! * `OXY_APP_CHECKS_GITHUB_API`: the GitHub API's base URL. Default
//!   `https://api.github.com`; a test points it at a loopback fake.
//!
//! The checker's own token is not here: it lives in GitHub, as the workflow's
//! secret, and the server holds only its hash (`OXY_SERVICE_TOKENS`).
//!
//! Permissive: nothing here refuses a boot. Without a token or a deployment
//! the runner is not started, and the checks stay queued for a person to run.

/// The deployments the workflow knows.
pub const DEPLOYMENTS: [&str; 3] = ["dev", "staging", "prod"];

const DEFAULT_REPO: &str = "oxy-hq/oxygen-internal";
const DEFAULT_WORKFLOW: &str = "custom-app-build-checks.yaml";
const DEFAULT_REF: &str = "main";
const DEFAULT_API: &str = "https://api.github.com";

/// The runner's configuration. `Debug` never prints the token.
#[derive(Clone, PartialEq, Eq)]
pub struct AppChecksConfig {
    pub github_token: Option<String>,
    pub github_repo: String,
    pub github_workflow: String,
    pub github_ref: String,
    pub github_api: String,
    /// As set: [`Self::dispatch`] says whether it is one of [`DEPLOYMENTS`].
    pub deployment: Option<String>,
}

/// What one dispatch needs, borrowed from a configuration that has all of it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DispatchConfig<'a> {
    pub token: &'a str,
    pub repo: &'a str,
    pub workflow: &'a str,
    pub git_ref: &'a str,
    pub api: &'a str,
    pub deployment: &'a str,
}

impl Default for AppChecksConfig {
    fn default() -> Self {
        Self::parse(&|_| None)
    }
}

impl AppChecksConfig {
    pub fn parse(var: &dyn Fn(&str) -> Option<String>) -> Self {
        let set = |name: &str| {
            var(name)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let or = |name: &str, default: &str| set(name).unwrap_or_else(|| default.to_string());
        Self {
            github_token: set("OXY_APP_CHECKS_GITHUB_TOKEN"),
            github_repo: or("OXY_APP_CHECKS_GITHUB_REPO", DEFAULT_REPO),
            github_workflow: or("OXY_APP_CHECKS_GITHUB_WORKFLOW", DEFAULT_WORKFLOW),
            github_ref: or("OXY_APP_CHECKS_GITHUB_REF", DEFAULT_REF),
            github_api: or("OXY_APP_CHECKS_GITHUB_API", DEFAULT_API)
                .trim_end_matches('/')
                .to_string(),
            deployment: set("OXY_APP_CHECKS_DEPLOYMENT"),
        }
    }

    /// What a dispatch needs, or why this deployment starts no runner: no
    /// token, no deployment, or a deployment the workflow does not know. The
    /// reason names the variable and never a value of the token.
    pub fn dispatch(&self) -> Result<DispatchConfig<'_>, String> {
        let Some(token) = self.github_token.as_deref() else {
            return Err("OXY_APP_CHECKS_GITHUB_TOKEN is not set".to_string());
        };
        let Some(deployment) = self.deployment.as_deref() else {
            return Err("OXY_APP_CHECKS_DEPLOYMENT is not set".to_string());
        };
        if !DEPLOYMENTS.contains(&deployment) {
            return Err(format!(
                "OXY_APP_CHECKS_DEPLOYMENT={deployment:?} is not one of {}",
                DEPLOYMENTS.join(", ")
            ));
        }
        Ok(DispatchConfig {
            token,
            repo: &self.github_repo,
            workflow: &self.github_workflow,
            git_ref: &self.github_ref,
            api: &self.github_api,
            deployment,
        })
    }
}

const NOT_SHOWN: &str = "(set, not shown)";

impl std::fmt::Debug for AppChecksConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppChecksConfig")
            .field(
                "github_token",
                &self.github_token.as_ref().map(|_| NOT_SHOWN),
            )
            .field("github_repo", &self.github_repo)
            .field("github_workflow", &self.github_workflow)
            .field("github_ref", &self.github_ref)
            .field("github_api", &self.github_api)
            .field("deployment", &self.deployment)
            .finish()
    }
}

impl std::fmt::Debug for DispatchConfig<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DispatchConfig")
            .field("token", &NOT_SHOWN)
            .field("repo", &self.repo)
            .field("workflow", &self.workflow)
            .field("git_ref", &self.git_ref)
            .field("api", &self.api)
            .field("deployment", &self.deployment)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "ghp_secretvalue";

    fn parse(pairs: &[(&str, &str)]) -> AppChecksConfig {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        AppChecksConfig::parse(&move |name| {
            owned
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
        })
    }

    #[test]
    fn unset_takes_the_defaults_and_starts_no_runner() {
        let cfg = parse(&[]);
        assert_eq!(cfg, AppChecksConfig::default());
        assert_eq!(cfg.github_repo, "oxy-hq/oxygen-internal");
        assert_eq!(cfg.github_workflow, "custom-app-build-checks.yaml");
        assert_eq!(cfg.github_ref, "main");
        assert_eq!(cfg.github_api, "https://api.github.com");
        assert_eq!(
            (cfg.github_token.clone(), cfg.deployment.clone()),
            (None, None)
        );
        assert_eq!(
            cfg.dispatch().unwrap_err(),
            "OXY_APP_CHECKS_GITHUB_TOKEN is not set"
        );
    }

    #[test]
    fn a_token_and_a_known_deployment_make_a_dispatch() {
        let padded = format!(" {SECRET} ");
        let cfg = parse(&[
            ("OXY_APP_CHECKS_GITHUB_TOKEN", &padded),
            ("OXY_APP_CHECKS_GITHUB_REPO", "acme/checks"),
            ("OXY_APP_CHECKS_GITHUB_WORKFLOW", "checks.yaml"),
            ("OXY_APP_CHECKS_GITHUB_REF", "release"),
            ("OXY_APP_CHECKS_GITHUB_API", "http://127.0.0.1:9/"),
            ("OXY_APP_CHECKS_DEPLOYMENT", "staging"),
        ]);
        let dispatch = cfg.dispatch().expect("configured");
        assert_eq!(dispatch.token, SECRET, "trimmed");
        assert_eq!(
            (dispatch.repo, dispatch.workflow, dispatch.git_ref),
            ("acme/checks", "checks.yaml", "release")
        );
        assert_eq!(dispatch.api, "http://127.0.0.1:9", "no trailing slash");
        assert_eq!(dispatch.deployment, "staging");
    }

    #[test]
    fn a_missing_or_unknown_deployment_starts_no_runner_and_blank_is_unset() {
        let token = ("OXY_APP_CHECKS_GITHUB_TOKEN", SECRET);
        let no_deployment = parse(&[token, ("OXY_APP_CHECKS_DEPLOYMENT", "  ")]);
        assert_eq!(no_deployment.deployment, None);
        assert_eq!(
            no_deployment.dispatch().unwrap_err(),
            "OXY_APP_CHECKS_DEPLOYMENT is not set"
        );
        let unknown = parse(&[token, ("OXY_APP_CHECKS_DEPLOYMENT", "production")]);
        let why = unknown.dispatch().unwrap_err();
        assert!(
            why.contains("\"production\"") && why.contains("dev, staging, prod"),
            "{why}"
        );
        assert!(!why.contains(SECRET));
        for known in DEPLOYMENTS {
            let cfg = parse(&[token, ("OXY_APP_CHECKS_DEPLOYMENT", known)]);
            assert_eq!(cfg.dispatch().unwrap().deployment, known);
        }
    }

    #[test]
    fn debug_never_prints_the_token() {
        let cfg = parse(&[
            ("OXY_APP_CHECKS_GITHUB_TOKEN", SECRET),
            ("OXY_APP_CHECKS_DEPLOYMENT", "dev"),
        ]);
        let shown = format!("{cfg:?} {:?}", cfg.dispatch().unwrap());
        assert!(!shown.contains(SECRET), "{shown}");
        assert_eq!(shown.matches("(set, not shown)").count(), 2, "{shown}");
        assert!(shown.contains("oxy-hq/oxygen-internal"), "{shown}");
        // The whole boot configuration prints through the same impl.
        let boot = crate::config::BootConfig::parse(&|name| {
            (name == "OXY_APP_CHECKS_GITHUB_TOKEN").then(|| SECRET.to_string())
        });
        assert!(!format!("{boot:?}").contains(SECRET));
    }
}
