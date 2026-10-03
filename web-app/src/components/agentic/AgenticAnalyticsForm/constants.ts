export const THINKING_OPTIONS = [
  { value: "disabled", label: "Disabled" },
  { value: "enabled", label: "Enabled" },
  { value: "adaptive", label: "Adaptive" },
  { value: "effort:low", label: "Effort: Low" },
  { value: "effort:medium", label: "Effort: Medium" },
  { value: "effort:high", label: "Effort: High" },
  // These two arrived at different versions: `max` on Claude 4.6, `xhigh` on
  // 4.7. Safe to pick on any *Claude* model, but by two different mechanisms:
  // on 4.6 `xhigh` is clamped to `high` and `output_config.effort` still goes
  // out, while below 4.6 the whole thinking config is sent as `budget_tokens`
  // and no `effort` field is written at all. Worth knowing if you are looking
  // for one in a request log. An unrecognised id (proxy alias, gateway) is
  // deliberately left alone and gets the level verbatim.
  { value: "effort:xhigh", label: "Effort: Extra High" },
  { value: "effort:max", label: "Effort: Max" }
] as const;

export const VENDOR_OPTIONS = [
  { value: "anthropic", label: "Anthropic" },
  { value: "openai", label: "OpenAI" },
  { value: "openai_compat", label: "OpenAI-compatible (Ollama, vLLM, …)" }
] as const;

/** `VendorKind` in `crates/agentic/analytics/src/config/yaml.rs`. */
export const SEMANTIC_ENGINE_VENDORS = [
  { value: "cube", label: "Cube" },
  { value: "looker", label: "Looker" }
] as const;

export const STATE_NAMES = [
  "clarifying",
  "specifying",
  "solving",
  "executing",
  "interpreting",
  "diagnosing"
] as const;
