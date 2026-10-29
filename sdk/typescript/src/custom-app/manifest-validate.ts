// `validateOxyAppManifest` — one validator for `oxy-app.json`, run by the SDK's
// loader, `oxyc validate`, `oxyc publish` and the Vite plugin.
//
// THREE LAYERS, ONE ANSWER. The JSON Schema (`manifest-schema.ts`, generated from
// the type) says what each field is; a few rules the schema cannot say — the
// v1 shapes, "at least one invocation surface" — are checked here; and the
// notification catalogue's own checks (`notification-catalog.ts`) run last, on
// the same manifest. Every finding carries a `path` a tool can point at and a
// `message` that names the field and the fix.
//
// ERRORS AND WARNINGS. A wrong type, a bad name, a missing required field is an
// error: the platform would refuse or misread it. A field the schema does not
// know is a WARNING — the server ignores what it does not read, and refusing a
// field a newer SDK added would break every app on a slightly older `oxyc` —
// but it is said, because a typo'd `timeoutSecond` is the commonest mistake a
// manifest holds, and silently ignored is how it used to be found: in
// production, at the first timeout.
//
// A SMALL WALKER, NOT A SCHEMA ENGINE. It covers exactly the keywords the
// generator emits — `manifest-schema.test.ts` pins that list — so this module
// carries no dependency and runs in a browser, a Vite config and `oxyc` alike.
// Nothing past ES2020.

import { FUNCTION_NAME_RULE, SLUG_RULE } from "./manifest-rules";
import { type ManifestSchemaNode, OXY_APP_SCHEMA } from "./manifest-schema";
import { checkNotifications } from "./notification-catalog";
import { checkWidgets } from "./widgets";

/** One finding about a manifest. */
export interface ManifestIssue {
  /** `error`: the platform refuses or misreads this. `warning`: it publishes, but is probably a mistake. */
  level: "error" | "warning";
  /** Dotted path from the root (`functions.sync.timeoutSeconds`), or `(root)`. */
  path: string;
  /** A sentence naming the field and what to change. */
  message: string;
}

/** The v1 fields a v2 manifest must not carry. */
const V1_FIELDS = ["products", "writers"] as const;

/** The message for a v1 manifest — the loader, the plugin and `oxyc` all say this. */
export const V1_MANIFEST_MESSAGE =
  "oxy-app.json is schemaVersion 2 (identity-only); the v1 fields `products` and `writers` are no longer supported";

const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/**
 * Closed sets the SERVER reads leniently: a value outside the set is logged
 * and dropped there, not refused, so refusing it here would be stricter than
 * the platform. Each is a warning that says what the server does instead.
 */
const LENIENT_ENUMS: ReadonlyArray<{ path: RegExp; because: string }> = [
  {
    // `RetentionPolicy::from_rules` (custom_apps_storage/retention.rs) warns
    // and skips the whole rule: a broader sibling rule then applies to the
    // prefix, else nothing does and the bytes are kept.
    path: /^storage\.retention\.\d+\.expireAfter$/,
    because:
      "the platform drops this rule with a warning, so a broader sibling rule applies to the prefix, else it never expires"
  }
];

/** The schema's answer about `raw`, as `ManifestIssue`s. Never throws. */
export function validateOxyAppManifest(raw: unknown): ManifestIssue[] {
  if (!isRecord(raw)) {
    return [{ level: "error", path: "(root)", message: "the top level must be an object" }];
  }
  const issues: ManifestIssue[] = [];

  // The two checks with their own sentences, ahead of the schema's: a v1
  // manifest gets told it is one, not that `schemaVersion` must equal 2.
  if (raw.schemaVersion !== 2) {
    issues.push({
      level: "error",
      path: "schemaVersion",
      message:
        `schemaVersion must be 2 (got ${JSON.stringify(raw.schemaVersion)}); ` +
        "v1 manifests are no longer supported — upgrade to the identity-only shape"
    });
  }
  const v1 = V1_FIELDS.some((field) => raw[field] !== undefined);
  if (v1) issues.push({ level: "error", path: "(root)", message: V1_MANIFEST_MESSAGE });

  const violations: Violation[] = [];
  walk(OXY_APP_SCHEMA, raw, [], violations);
  for (const violation of violations) {
    const path = violation.path.join(".");
    if (path === "schemaVersion") continue;
    if (v1 && violation.kind === "unknown" && (V1_FIELDS as readonly string[]).includes(path)) {
      continue;
    }
    const lenient = violation.kind === "enum" && LENIENT_ENUMS.find((l) => l.path.test(path));
    if (lenient) {
      issues.push({
        level: "warning",
        path,
        message: `${describe(violation)} — ${lenient.because}`
      });
      continue;
    }
    issues.push({
      level: violation.kind === "unknown" ? "warning" : "error",
      path,
      message: describe(violation)
    });
  }

  issues.push(...surfaceRule(raw));

  // The catalogue's checks know more about `notifications` than the schema does
  // (which categories exist, which kinds a function sends), so where both speak
  // about one path the catalogue's sentence is the one kept.
  // `checkWidgets` does the same for `widgets`.
  const catalogue = [...checkNotifications(raw), ...checkWidgets(raw)];
  const said = new Set(catalogue.map((f) => f.path));
  const kept = issues.filter((issue) => !said.has(issue.path));
  for (const f of catalogue) kept.push({ level: f.level, path: f.path, message: f.message });

  return dedupe(kept);
}

/** `true` when `raw` is a v1 manifest: the shape a migration guide answers. */
export function isV1Manifest(raw: unknown): boolean {
  return (
    isRecord(raw) &&
    (raw.schemaVersion !== 2 || V1_FIELDS.some((field) => raw[field] !== undefined))
  );
}

/** One error per path, the first said; warnings are kept as they come. */
function dedupe(issues: ManifestIssue[]): ManifestIssue[] {
  const seen = new Set<string>();
  const out: ManifestIssue[] = [];
  for (const issue of issues) {
    if (issue.level === "error") {
      if (seen.has(issue.path)) continue;
      seen.add(issue.path);
    }
    out.push(issue);
  }
  return out;
}

/**
 * At least one of `route` / `schedule` / `airwayStep` / `webhook` must be active
 * — `route` defaults to true only when no other surface is declared. A function
 * nothing can invoke is a bundle entry the author forgot to wire.
 */
function surfaceRule(raw: Record<string, unknown>): ManifestIssue[] {
  if (!isRecord(raw.functions)) return [];
  const out: ManifestIssue[] = [];
  for (const [name, fn] of Object.entries(raw.functions)) {
    if (!isRecord(fn)) continue;
    const other =
      fn.schedule !== undefined || fn.airwayStep !== undefined || fn.webhook !== undefined;
    const route = typeof fn.route === "boolean" ? fn.route : !other;
    if (!route && !other) {
      out.push({
        level: "error",
        path: `functions.${name}`,
        message: `function ${JSON.stringify(name)} must enable at least one of route/schedule/airwayStep/webhook`
      });
    }
  }
  return out;
}

// ── The walker ──────────────────────────────────────────────────────────────

type ViolationKind =
  | "type"
  | "required"
  | "unknown"
  | "enum"
  | "const"
  | "pattern"
  | "minLength"
  | "maxLength"
  | "range"
  | "propertyName"
  | "anyOf";

interface Violation {
  kind: ViolationKind;
  path: string[];
  /** The schema node the value failed, for the sentence. */
  node: ManifestSchemaNode;
  /** The value, where the sentence quotes it. */
  value: unknown;
}

function resolve(node: ManifestSchemaNode): ManifestSchemaNode {
  if (node.$ref === undefined) return node;
  const name = node.$ref.replace(/^#\/definitions\//, "");
  const target = OXY_APP_SCHEMA.definitions[name];
  if (target === undefined) throw new Error(`oxy-app.schema.json: unresolved $ref ${node.$ref}`);
  // A `$ref` beside a `description` (how the generator annotates a property):
  // the description is the property's, the shape is the target's.
  return target;
}

function matchesType(type: string, value: unknown): boolean {
  switch (type) {
    case "object":
      return isRecord(value);
    case "array":
      return Array.isArray(value);
    case "string":
      return typeof value === "string";
    case "boolean":
      return typeof value === "boolean";
    case "number":
      return typeof value === "number" && Number.isFinite(value);
    case "integer":
      return typeof value === "number" && Number.isInteger(value);
    case "null":
      return value === null;
    default:
      return true;
  }
}

function walk(raw: ManifestSchemaNode, value: unknown, path: string[], out: Violation[]): void {
  const node = resolve(raw);
  const fail = (kind: ViolationKind, at: string[] = path): void => {
    out.push({ kind, path: at, node, value });
  };

  if (node.type !== undefined) {
    const types = Array.isArray(node.type) ? node.type : [node.type];
    if (!types.some((t) => matchesType(t, value))) {
      fail("type");
      return;
    }
  }
  if (node.enum !== undefined && !node.enum.some((allowed) => allowed === value)) {
    fail("enum");
    return;
  }
  if (node.const !== undefined && node.const !== value) {
    fail("const");
    return;
  }
  if (node.anyOf !== undefined) {
    const fits = node.anyOf.some((branch) => {
      const scratch: Violation[] = [];
      walk(branch, value, path, scratch);
      return scratch.length === 0;
    });
    if (!fits) {
      fail("anyOf");
      return;
    }
  }

  if (typeof value === "string") {
    if (node.pattern !== undefined && !new RegExp(node.pattern).test(value)) fail("pattern");
    else if (node.minLength !== undefined && value.length < node.minLength) fail("minLength");
    else if (node.maxLength !== undefined && value.length > node.maxLength) fail("maxLength");
    return;
  }
  if (typeof value === "number") {
    if (
      (node.minimum !== undefined && value < node.minimum) ||
      (node.maximum !== undefined && value > node.maximum)
    ) {
      fail("range");
    }
    return;
  }
  if (Array.isArray(value)) {
    if (node.items !== undefined) {
      value.forEach((item, i) =>
        walk(node.items as ManifestSchemaNode, item, [...path, String(i)], out)
      );
    }
    return;
  }
  if (!isRecord(value)) return;

  const properties = node.properties ?? {};
  for (const name of node.required ?? []) {
    if (value[name] === undefined) fail("required", [...path, name]);
  }
  for (const [name, member] of Object.entries(value)) {
    if (member === undefined) continue;
    const at = [...path, name];
    const declared = properties[name];
    if (declared !== undefined) {
      walk(declared, member, at, out);
      continue;
    }
    const pattern = node.propertyNames?.pattern;
    if (pattern !== undefined && !new RegExp(pattern).test(name)) {
      fail("propertyName", at);
      continue;
    }
    const extra = node.additionalProperties;
    if (extra === false) fail("unknown", at);
    else if (isRecord(extra)) walk(extra as ManifestSchemaNode, member, at, out);
  }
}

// ── Sentences ───────────────────────────────────────────────────────────────

/** `function "sync" \`timeoutSeconds\``, or `\`build.outDir\``, or `the top level`. */
function label(path: string[]): string {
  if (path.length === 0) return "the top level";
  if (path[0] === "functions" && path.length >= 2) {
    const fn = JSON.stringify(path[1]);
    const rest = path.slice(2).join(".");
    return rest === "" ? `function ${fn}` : `function ${fn} \`${rest}\``;
  }
  return `\`${path.join(".")}\``;
}

const ARTICLES: Record<string, string> = {
  object: "an object",
  array: "an array",
  string: "a string",
  boolean: "a boolean",
  number: "a number",
  integer: "an integer",
  null: "null"
};

function typePhrase(node: ManifestSchemaNode): string {
  const types = (Array.isArray(node.type) ? node.type : [node.type ?? ""]).filter((t) => t !== "");
  const named = types.map((t) => ARTICLES[t] ?? t);
  if (named.length === 0) return "a value the schema allows";
  const main = named.filter((n) => n !== "null");
  const base = (main.length > 0 ? main : named).join(" or ");
  return main.length < named.length ? `${base} or null` : base;
}

function isNumeric(node: ManifestSchemaNode): boolean {
  return node.type === "integer" || node.type === "number";
}

/** `an integer in [1, 300]`, `a positive integer`, `an integer of at least 1`. */
function numberPhrase(node: ManifestSchemaNode): string {
  const noun = node.type === "integer" ? "integer" : "number";
  const { minimum, maximum } = node;
  if (minimum !== undefined && maximum !== undefined)
    return `an ${noun} in [${minimum}, ${maximum}]`;
  if (minimum === 1 && noun === "integer") return "a positive integer";
  if (minimum !== undefined) return `${ARTICLES[noun]} of at least ${minimum}`;
  if (maximum !== undefined) return `${ARTICLES[noun]} of at most ${maximum}`;
  return ARTICLES[noun] ?? noun;
}

function oneOf(values: unknown[]): string {
  const quoted = values.map((v) => JSON.stringify(v));
  if (quoted.length <= 1) return quoted.join("");
  return `${quoted.slice(0, -1).join(", ")} or ${quoted[quoted.length - 1]}`;
}

function describe(v: Violation): string {
  const at = v.path.join(".");
  const who = label(v.path);

  if (at === "slug") {
    if (typeof v.value !== "string" || v.value.trim() === "") {
      return "`slug` is required and must be a non-empty string";
    }
    return (
      `\`slug\` ${JSON.stringify(v.value)} is invalid — use ${SLUG_RULE}; it becomes the app's ` +
      "served path and OLTP schema name, so a malformed slug is refused at publish"
    );
  }
  if (at === "functions" && v.kind === "type") {
    return "functions must be an object keyed by function name";
  }

  switch (v.kind) {
    case "propertyName":
      return (
        `function name ${JSON.stringify(v.path[v.path.length - 1])} must match ` +
        `${v.node.propertyNames?.pattern ?? ""} — ${FUNCTION_NAME_RULE}; a malformed name cannot become functions/<name>.js`
      );
    case "required":
      return `${who} is required`;
    case "unknown":
      return `unknown field ${who}: the platform ignores what it does not read, so check the spelling against the schema`;
    case "type":
      // A numeric field says its range on a wrong type too: "an integer in
      // [1, 300]" is the whole rule, where "a number" is half of it.
      return `${who} must be ${isNumeric(v.node) ? numberPhrase(v.node) : typePhrase(v.node)}`;
    case "range":
      return `${who} must be ${numberPhrase(v.node)}`;
    case "enum":
      return `${who} must be ${oneOf(v.node.enum ?? [])}`;
    case "const":
      return `${who} must be ${JSON.stringify(v.node.const)}`;
    case "pattern":
      return v.node.pattern === "\\S"
        ? `${who} must be a non-empty string`
        : `${who} must match ${v.node.pattern ?? ""}`;
    case "minLength":
      return v.node.minLength === 1
        ? `${who} must be a non-empty string`
        : `${who} must be at least ${v.node.minLength} characters`;
    case "maxLength":
      return `${who} must be at most ${v.node.maxLength} characters`;
    case "anyOf":
      return `${who} does not match any shape the schema allows`;
    default: {
      const unreachable: never = v.kind;
      return unreachable;
    }
  }
}
