// Secret handling — keep `${VAR}` placeholders in everything that gets
// stored on disk or uploaded as an artifact. Substitute the real values
// only at the two egress boundaries:
//
//   1. Just before the step prompt is sent to the Anthropic API.
//   2. Just before a state-changing tool (browser_type / browser_keyboard_type)
//      is dispatched against Playwright.
//
// Recorded actions, debug tool args, and step.text all stay redacted, so
// the action cache (`.cache/bespoke-actions.json`) and the result artifact
// (`.results/<ts>.json`) never contain plaintext secrets.

const PLACEHOLDER_RE = /\$\{([A-Z_][A-Z0-9_]*)\}/g;

// Allowlist of env vars treated as secret. Keep narrow so we don't get
// false-positive substitutions on common short strings (a non-secret
// "localhost" wouldn't match anything here).
const SECRET_ENV_VARS = [
  "ANTHROPIC_API_KEY",
  "OPENAI_API_KEY",
  "GEMINI_API_KEY",
  "CLICKHOUSE_PASSWORD",
  "OXY_DATABASE_URL",
  // Not a secret: the release showcase's per-pass token (tests/showcase/
  // capture.ts). Listed so a typed "Front counter 3f9a2c1b" records as
  // "Front counter ${SHOWCASE_RUN}", and each replay creates a new one rather
  // than colliding with the row the previous pass made.
  "SHOWCASE_RUN"
] as const;

// Minimum length for an env-var value to be treated as redactable. Below
// this we treat the value as a test stub (e.g. `GEMINI_API_KEY=empty`)
// and skip — substituting "empty" everywhere would corrupt unrelated
// arg strings.
const MIN_SECRET_LENGTH = 8;

interface SecretEntry {
  name: string;
  value: string;
}

function getSecretEntries(): SecretEntry[] {
  const out: SecretEntry[] = [];
  for (const name of SECRET_ENV_VARS) {
    const value = process.env[name];
    if (value && value.length >= MIN_SECRET_LENGTH) {
      out.push({ name, value });
    }
  }
  // Sort longest-first so multi-secret strings redact correctly even when
  // one secret value happens to be a substring of another.
  out.sort((a, b) => b.value.length - a.value.length);
  return out;
}

/**
 * Validate `${VAR}` placeholders in `text` against the current environment.
 * Throws on a missing variable. Used by the YAML loader at flow-load time
 * so authors fail loudly before anything reaches the LLM.
 */
export function validatePlaceholders(text: string, where: string): void {
  for (const m of text.matchAll(PLACEHOLDER_RE)) {
    const name = m[1];
    if (process.env[name] === undefined) {
      throw new Error(
        `${where}: environment variable \${${name}} referenced in step text but not set`
      );
    }
  }
}

/**
 * Replace `${VAR}` placeholders in `text` with the env value. Throws if
 * any referenced variable is unset. Used at egress boundaries (sending
 * prompts to the LLM, dispatching tool calls to Playwright).
 */
export function expandSecrets(text: string): string {
  return text.replace(PLACEHOLDER_RE, (_, name: string) => {
    const value = process.env[name];
    if (value === undefined) {
      throw new Error(`environment variable \${${name}} referenced but not set`);
    }
    return value;
  });
}

/**
 * Replace any plaintext occurrence of an allowlisted env-var value in
 * `text` with `${VAR}`. Used at the recording boundary so cached actions
 * and debug tool args never persist plaintext.
 */
export function redactSecrets(text: string): string {
  let result = text;
  for (const { name, value } of getSecretEntries()) {
    if (result.includes(value)) {
      result = result.split(value).join(`\${${name}}`);
    }
  }
  return result;
}

/**
 * Walk a tool-call args object and redact any string value that contains
 * an allowlisted env-var value. Non-string values pass through unchanged.
 * Used before recording into the action cache or debug tool_calls list.
 *
 * Defense in depth: after redaction, asserts that no allowlisted plaintext
 * value still appears in any string. This should never fire — if it does,
 * the allowlist is incomplete or redactSecrets() has a bug, and the
 * caller should hear about it before the value reaches disk.
 */
export function redactArgs(args: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(args)) {
    out[k] = typeof v === "string" ? redactSecrets(v) : v;
  }
  for (const [k, v] of Object.entries(out)) {
    if (typeof v !== "string") continue;
    for (const { name, value } of getSecretEntries()) {
      if (v.includes(value)) {
        throw new Error(
          `secret redaction failed: arg '${k}' still contains plaintext value of env var \${${name}} after redactSecrets — refusing to record. Please report this as a bug.`
        );
      }
    }
  }
  return out;
}

/**
 * Walk a tool-call args object and expand `${VAR}` placeholders in string
 * values. Used before dispatching a cached action to Playwright.
 */
export function expandArgs(args: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(args)) {
    out[k] = typeof v === "string" ? expandSecrets(v) : v;
  }
  return out;
}

/**
 * Selector strategies are read off the live DOM after an action — the text and
 * accessible name of what was clicked — so they carry whatever the page shows,
 * a value the model typed from a `${VAR}` included. They cross the same two
 * boundaries args do: redacted before they are recorded, expanded before a
 * replay dispatches them. Without the second half a replay dispatched the
 * literal `${VAR}`; without the first, the record pass's own value, which
 * points a replay at the row the record pass created.
 */
export function redactStrategies<T extends { selector: string }>(strategies: T[]): T[] {
  return strategies.map((s) => ({ ...s, selector: redactSecrets(s.selector) }));
}

export function expandStrategies<T extends { selector: string }>(strategies: T[]): T[] {
  return strategies.map((s) => ({ ...s, selector: expandSecrets(s.selector) }));
}
