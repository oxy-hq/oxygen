/**
 * What the runtime's view and topic parser checks beyond the shape.
 *
 * A port of `oxy_semantic::SemanticLayerParser::parse_view_str` /
 * `parse_topic_str` (the `oxy-semantic` crate), the parser the Rust
 * `oxy validate` ran, minus the deserialization, which `view.json` /
 * `topic.json` carry. Two checks:
 *
 * - `reject_legacy_globals` on the raw text, BEFORE the YAML parse, as Rust
 *   does: `inherits_from:` and `{{ globals.* }}` are removed features that
 *   otherwise parse and silently mean nothing.
 * - `validate_variable_syntax` on the fields Rust runs it on: for a view, each
 *   dimension's and measure's `expr`, each measure filter's `expr`, `table` and
 *   `sql`; for a topic, every string anywhere.
 *
 * Rust classes the first as a parsing error and the second as a validation
 * error, so the findings carry `parse` and `rule`.
 */

import type { Finding } from "./finding.js";

/** `reject_legacy_globals`: the finding, or `undefined` when the text is clean. */
export function legacyGlobalsFinding(file: string, raw: string): Finding | undefined {
  const inherits = raw.split("\n").some((line) => line.trimStart().startsWith("inherits_from:"));
  const globals = raw.includes("{{globals.") || raw.includes("{{ globals.");
  if (!inherits && !globals) return undefined;
  return {
    file,
    path: "(root)",
    kind: "parse",
    message:
      "uses removed semantic-model features (`inherits_from:` and/or `{{ globals.* }}`). The " +
      "semantics.yml registry has been removed; inline the referenced values directly into the file."
  };
}

const VALID_VARIABLE = /\{\{variables\.[a-zA-Z_][a-zA-Z0-9_.]*\}\}/;
const VARIABLE_LIKE = /\{\{variables[^}]*\}\}/g;

/** `validate_variable_syntax`: one message per problem in `expr`. */
export function variableSyntaxErrors(expr: string, context: string): string[] {
  const errors: string[] = [];
  for (const match of expr.matchAll(VARIABLE_LIKE)) {
    if (!VALID_VARIABLE.test(match[0])) {
      errors.push(
        `${context} contains invalid variable syntax: '${match[0]}'. ` +
          "Variables must use format: {{variables.name}}"
      );
    }
  }
  if (expr.includes("{{variables") && !expr.includes("{{variables.")) {
    errors.push(
      `${context} contains incomplete variable expression. ` +
        "Variables must include a name after 'variables.'"
    );
  }
  return errors;
}

function record(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function rule(file: string, path: string, message: string): Finding {
  return { file, path, kind: "rule", message };
}

/** The variable checks `preprocess_variables` makes on a parsed view. */
export function viewVariableFindings(file: string, view: unknown): Finding[] {
  const doc = record(view);
  if (!doc) return [];
  const out: Finding[] = [];
  const check = (value: unknown, path: string, context: string) => {
    if (typeof value !== "string") return;
    for (const m of variableSyntaxErrors(value, context)) out.push(rule(file, path, m));
  };

  const dimensions = Array.isArray(doc.dimensions) ? doc.dimensions : [];
  dimensions.forEach((d, i) => {
    check(record(d)?.expr, `dimensions.${i}.expr`, "Dimension");
  });
  const measures = Array.isArray(doc.measures) ? doc.measures : [];
  measures.forEach((m, i) => {
    const measure = record(m);
    check(measure?.expr, `measures.${i}.expr`, "Measure");
    const filters = Array.isArray(measure?.filters) ? measure.filters : [];
    filters.forEach((f, j) => {
      check(record(f)?.expr, `measures.${i}.filters.${j}.expr`, "Measure filter");
    });
  });
  check(doc.table, "table", "Table reference");
  check(doc.sql, "sql", "SQL query");
  return out;
}

/** `validate_topic_variables`: every string in a parsed topic. */
export function topicVariableFindings(file: string, topic: unknown): Finding[] {
  const out: Finding[] = [];
  walkStrings(topic, "", (value, path) => {
    for (const m of variableSyntaxErrors(value, "Topic")) out.push(rule(file, path || "(root)", m));
  });
  return out;
}

/** Call `visit` with every string under `value`, and its dotted path. */
function walkStrings(value: unknown, path: string, visit: (s: string, path: string) => void): void {
  const at = (key: string | number) => (path ? `${path}.${key}` : `${key}`);
  if (typeof value === "string") visit(value, path);
  const children = Array.isArray(value) ? value.entries() : Object.entries(record(value) ?? {});
  if (typeof value !== "string") for (const [key, v] of children) walkStrings(v, at(key), visit);
}
