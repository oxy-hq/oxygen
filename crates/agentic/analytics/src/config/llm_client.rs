//! The agent's model and thinking settings, resolved once for both runtimes.
//!
//! The FSM solver and the react loop read the same `AgentConfig` through these
//! functions, so the comparison between them runs on one model, one key and
//! one thinking setting by construction.

use chrono_tz::Tz;

use super::{
    AgentConfig, BuildContext, ConfigError, LlmClientSpec, LlmVendor, ResolvedModelInfo,
    build_llm_client, resolve_timezone, resolve_vendor,
};
use crate::llm::{DEFAULT_MODEL, LlmClient, ThinkingConfig};
use crate::solver::select_thinking;

impl ResolvedModelInfo {
    /// The borrowed spec [`build_llm_client`] takes.
    pub(crate) fn client_spec(&self) -> LlmClientSpec<'_> {
        LlmClientSpec {
            vendor: &self.vendor,
            api_key: self.api_key.as_deref().unwrap_or(""),
            model: &self.model,
            base_url: self.base_url.as_deref(),
            azure_deployment_id: self.azure_deployment_id.as_deref(),
            azure_api_version: self.azure_api_version.as_deref(),
            headers: self.headers.as_ref(),
        }
    }
}

impl AgentConfig {
    /// The model the agent's primary client talks to.
    ///
    /// Precedence (highest → lowest):
    ///   a. Explicit `llm:` fields in the agent YAML.
    ///   b. Project model resolved from config.yml (`pmi`).
    ///   c. Environment variables / built-in defaults.
    ///
    /// When `llm.model` is absent in the YAML the project model config
    /// (vendor, model_ref, api_key, base_url) is used wholesale. Azure fields
    /// and custom headers come from the project model config only.
    pub fn effective_model_info(&self, pmi: Option<&ResolvedModelInfo>) -> ResolvedModelInfo {
        let model = self
            .llm
            .model
            .as_deref()
            .or(pmi.map(|m| m.model.as_str()))
            .unwrap_or(DEFAULT_MODEL)
            .to_string();
        // See `resolve_vendor`: an explicit `llm.vendor` wins over a `ref`.
        let vendor = resolve_vendor(self.llm.vendor.as_ref(), pmi.map(|m| &m.vendor)).clone();
        // API key: YAML → project resolved key → vendor env var.
        let api_key = self
            .llm
            .api_key
            .clone()
            .or_else(|| pmi.and_then(|m| m.api_key.clone()))
            .or_else(|| match vendor {
                LlmVendor::Anthropic => std::env::var("ANTHROPIC_API_KEY").ok(),
                LlmVendor::OpenAi | LlmVendor::OpenAiCompat => std::env::var("OPENAI_API_KEY").ok(),
            })
            .unwrap_or_default();
        let base_url = self
            .llm
            .base_url
            .clone()
            .or_else(|| pmi.and_then(|m| m.base_url.clone()));
        ResolvedModelInfo {
            model,
            vendor,
            api_key: Some(api_key),
            base_url,
            azure_deployment_id: pmi.and_then(|m| m.azure_deployment_id.clone()),
            azure_api_version: pmi.and_then(|m| m.azure_api_version.clone()),
            headers: pmi.and_then(|m| m.headers.clone()),
        }
    }

    /// A state's own `model:` override. The FSM's per-state clients are built
    /// from it, and so is the react client (through [`Self::state_model`]).
    pub(crate) fn state_own_model(&self, state: &str) -> Option<&str> {
        self.states.get(state).and_then(|c| c.model.as_deref())
    }

    /// The model a state's client calls: the UI override (which replaces
    /// every per-state client, `with_client_override`), else the state's own
    /// `model:`, else `None` (the agent's model).
    pub(crate) fn state_model<'a>(&'a self, state: &str, ctx: &'a BuildContext) -> Option<&'a str> {
        ctx.model_override
            .as_deref()
            .or_else(|| self.state_own_model(state))
    }

    /// What the FSM's `solving` state calls: the effective model with the
    /// solving state's model applied ([`Self::state_model`]).
    pub fn solving_model_info(&self, ctx: &BuildContext) -> ResolvedModelInfo {
        let mut info = self.effective_model_info(ctx.project_model_info.as_ref());
        if let Some(model) = self.state_model("solving", ctx) {
            info.model = model.to_string();
        }
        info
    }

    /// The client the FSM's `solving` state calls (`client_for_state`),
    /// stamped with `ctx.genai`.
    pub fn solving_llm_client(&self, ctx: &BuildContext) -> LlmClient {
        let info = self.solving_model_info(ctx);
        build_llm_client(info.client_spec()).with_genai_context(ctx.genai.clone())
    }

    /// The thinking setting the FSM's `solving` state runs with: the UI
    /// override, then `states.solving.thinking`, then `llm.thinking` /
    /// top-level `thinking`, then adaptive.
    pub fn solving_thinking(&self, ctx: &BuildContext) -> ThinkingConfig {
        let extended = ctx.thinking_override.is_some() || ctx.model_override.is_some();
        let global = ctx
            .thinking_override
            .clone()
            .or_else(|| self.global_thinking());
        let state = self
            .states
            .get("solving")
            .and_then(|c| c.thinking.as_ref())
            .map(|t| t.to_thinking_config());
        select_thinking(extended, state, global.as_ref(), ThinkingConfig::Adaptive)
    }

    /// The `(thinking_override, model_override)` a run with the UI's
    /// extended-thinking toggle on applies: the agent's
    /// `llm.extended_thinking.{thinking, model}`. Both `None` when the toggle
    /// is off or the agent declares no `extended_thinking`. The FSM's
    /// `start_pipeline` and the served react build both read it.
    pub fn extended_thinking_overrides(
        &self,
        extended: bool,
    ) -> (Option<ThinkingConfig>, Option<String>) {
        let ext = self.llm.extended_thinking.as_ref().filter(|_| extended);
        ext.map_or((None, None), |ext| {
            (
                ext.thinking.as_ref().map(|t| t.to_thinking_config()),
                ext.model.clone(),
            )
        })
    }

    /// `llm.thinking`, else the top-level `thinking`.
    pub(crate) fn global_thinking(&self) -> Option<ThinkingConfig> {
        let yaml = self.llm.thinking.as_ref().or(self.thinking.as_ref());
        yaml.map(|t| t.to_thinking_config())
    }

    /// The date-hint timezone: the agent's `timezone:`, else the project's.
    /// An invalid agent value is an error, as it is for the FSM.
    pub fn effective_timezone(&self, project: Option<Tz>) -> Result<Option<Tz>, ConfigError> {
        resolve_timezone(self.timezone.as_deref(), project)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(yaml: &str) -> AgentConfig {
        AgentConfig::from_yaml(yaml).unwrap()
    }

    #[test]
    fn extended_thinking_overrides_apply_only_when_the_run_asks() {
        let cfg = agent(
            "llm:\n  model: base\n  extended_thinking:\n    model: deep\n    thinking: disabled\n",
        );
        let (thinking, model) = cfg.extended_thinking_overrides(true);
        assert!(matches!(thinking, Some(ThinkingConfig::Disabled)));
        assert_eq!(model.as_deref(), Some("deep"));
        let (thinking, model) = cfg.extended_thinking_overrides(false);
        assert!(thinking.is_none() && model.is_none());
        let (thinking, model) = agent("").extended_thinking_overrides(true);
        assert!(
            thinking.is_none() && model.is_none(),
            "no extended_thinking block"
        );
    }

    #[test]
    fn solving_thinking_defaults_to_adaptive_like_the_fsm_solving_state() {
        let ctx = BuildContext::default();
        assert!(matches!(
            agent("").solving_thinking(&ctx),
            ThinkingConfig::Adaptive
        ));
        let off = agent("thinking: disabled\n");
        assert!(matches!(
            off.solving_thinking(&ctx),
            ThinkingConfig::Disabled
        ));
    }

    #[test]
    fn solving_thinking_prefers_the_solving_state_then_the_ui_override() {
        let cfg = agent("thinking: adaptive\nstates:\n  solving:\n    thinking: disabled\n");
        let mut ctx = BuildContext::default();
        assert!(matches!(
            cfg.solving_thinking(&ctx),
            ThinkingConfig::Disabled
        ));
        ctx.thinking_override = Some(ThinkingConfig::Adaptive);
        assert!(matches!(
            cfg.solving_thinking(&ctx),
            ThinkingConfig::Adaptive
        ));
    }

    #[test]
    fn solving_model_info_takes_the_solving_state_model_then_the_ui_override() {
        let cfg =
            agent("llm:\n  model: agent-model\nstates:\n  solving:\n    model: solve-model\n");
        let mut ctx = BuildContext::default();
        assert_eq!(cfg.solving_model_info(&ctx).model, "solve-model");
        assert_eq!(
            agent("llm:\n  model: agent-model\n")
                .solving_model_info(&ctx)
                .model,
            "agent-model"
        );
        ctx.model_override = Some("ui-model".into());
        assert_eq!(cfg.solving_model_info(&ctx).model, "ui-model");
    }

    #[test]
    fn effective_model_info_lets_the_yaml_model_win_over_the_project_ref() {
        let pmi = ResolvedModelInfo {
            model: "project-model".into(),
            vendor: LlmVendor::Anthropic,
            api_key: Some("k".into()),
            base_url: None,
            azure_deployment_id: None,
            azure_api_version: None,
            headers: None,
        };
        let info = agent("llm:\n  model: yaml-model\n").effective_model_info(Some(&pmi));
        assert_eq!(info.model, "yaml-model");
        assert_eq!(info.api_key.as_deref(), Some("k"));
        let inherited = agent("").effective_model_info(Some(&pmi));
        assert_eq!(inherited.model, "project-model");
    }
}
