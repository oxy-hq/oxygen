// Low-durability lint for flow YAMLs. Run during `--dry-run` and
// surfaced in the markdown summary. Warns (does not fail) when an `act:`
// or `expect.assert:` step uses selectors that drift on routine UI churn:
//
//   - `text=...` only — breaks on copy edits, i18n, label tweaks.
//   - bare CSS class selectors (`.foo button`) — breaks on Tailwind /
//     restructure refactors.
//
// The recommended form is `[data-testid=...]`. The runtime records 2–3
// fallback strategies per state-changing tool, so even a fragile
// primary survives single-axis drift; but a recording that never lands
// on a testid as primary defeats the durability story altogether.

import type { FlowTest } from "./types";

type LintSeverity = "warn" | "info";

export interface LintFinding {
  flow: string;
  case: string;
  step_index: number;
  severity: LintSeverity;
  rule: string;
  message: string;
}

const TESTID_RE = /\[data-testid[=~]/;
const ROLE_RE = /role=[a-z]+(\[name=)?/i;
const TEXT_ONLY_RE = /\btext=/;
const CSS_CLASS_RE = /[.#][a-zA-Z][\w-]*\s+\w+/; // `.foo button`, `#bar input`
// `\b` before `\[data-testid` never fires (space + `[` are both non-word
// chars) — drop it so a properly-quoted testid in the act prompt counts
// as a selector hint. Same for `\bbrowser_file_upload`-style additions.
//
// `navigate` belongs in this list even though it takes no selector — BECAUSE
// it takes no selector. The rule exists to catch a step that makes the model
// hunt the snapshot for its target; a `browser_navigate` act names a URL and
// is the most deterministic step the runtime has. Without it here, the three
// navigate acts in `custom-app-console-health` (which exist precisely because
// the console's page rail is `lg:hidden` when railed, so clicking it burned 12
// iterations and $1.00) were each reported as fragile, and the deep-link act
// in `customer-apps-oxy-starter-fleet` was one of the pre-existing findings
// the skill tells people to ignore. A lint that cries wolf about the right
// answer teaches people to ignore it.
const SELECTOR_HINT_RE =
  /(\bbrowser_(click|type|press_key|fill|hover|select|file_upload|navigate)\b|selector\s|\[data-testid|\brole=|\bcss=|\btext=|#[a-z][\w-]+|\.[a-z][\w-]+)/i;

export function lintFlow(flow: FlowTest): LintFinding[] {
  const findings: LintFinding[] = [];
  for (const c of flow.cases) {
    for (let i = 0; i < c.steps.length; i++) {
      const step = c.steps[i];
      if (!step.act) continue;
      const text = step.act;
      const hasTestId = TESTID_RE.test(text);
      const hasRole = ROLE_RE.test(text);
      const hasTextSel = TEXT_ONLY_RE.test(text);
      const hasCssSel = CSS_CLASS_RE.test(text);
      const hasAnySelector = SELECTOR_HINT_RE.test(text);

      if (!hasAnySelector) {
        findings.push({
          flow: flow.name,
          case: c.name,
          step_index: i,
          severity: "info",
          rule: "no-selector-hint",
          message:
            "Step has no explicit selector or browser_ tool hint. The LLM will discover the target from the snapshot, which produces fragile recordings. Quote `[data-testid=…]` if you have one."
        });
        continue;
      }

      if (!hasTestId && hasTextSel && !hasRole) {
        findings.push({
          flow: flow.name,
          case: c.name,
          step_index: i,
          severity: "warn",
          rule: "text-only-selector",
          message:
            "Step relies on `text=` only. Drifts on copy edits, i18n, or label tweaks. Prefer `[data-testid=…]`; fall back to `role=…[name=…]` if no testid is available."
        });
      }

      if (!hasTestId && !hasRole && hasCssSel) {
        findings.push({
          flow: flow.name,
          case: c.name,
          step_index: i,
          severity: "warn",
          rule: "css-structure-selector",
          message:
            "Step uses a CSS class/structure selector. Drifts on Tailwind refactors and component restructures. Prefer `[data-testid=…]`."
        });
      }
    }
  }
  return findings;
}

/** One-line summary of findings, suitable for CLI / GITHUB_STEP_SUMMARY. */
export function formatFindings(findings: LintFinding[]): string {
  if (findings.length === 0) return "No durability findings.";
  const lines = [`Durability findings (${findings.length}):`];
  for (const f of findings) {
    lines.push(
      `  [${f.severity}] ${f.flow} → ${f.case} step[${f.step_index}] (${f.rule}): ${f.message}`
    );
  }
  return lines.join("\n");
}
