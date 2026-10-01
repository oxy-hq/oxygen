/**
 * `env.<KEY>.shared` in `oxy-app.json`: outside production, `ctx.env` falls
 * back to production's value for a shared key while the environment holds none
 * of its own. So a shared key must be one nothing writes — the server refuses
 * the publish otherwise (`custom_apps_secrets::shared_env::check_shared_env`),
 * and this answers first, in `oxyc validate`:
 *
 * - a key a function sets with `ctx.secrets.set("<KEY>", …)` is rotated state
 *   (a refreshed OAuth token), and staging rotating production's token would
 *   fork production's grant at the provider;
 * - a `webhook.secretVar` verifies deliveries, and staging's webhook route
 *   verifies with staging's own value.
 *
 * ADVISORY, like the rest of `placement`: only a string-literal key in a
 * function's entry file is seen, and the server is the authority.
 */

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { functionEntry, type PublishManifest } from "./manifest.js";
import type { PlacementIssue } from "./placement.js";
import { lineAt, literalString, maskJs, splitArgs } from "./placement-scan.js";

const MANIFEST = "oxy-app.json";
/** `secrets.set(` / `secrets?.set(` — the plain and minified chains. */
const SECRETS_SET = /\bsecrets\s*\??\.\s*set\s*\(/g;
/** `[…].set(` — checked below for a `"secrets"` literal inside the brackets. */
const BRACKET_SET = /\[([^[\]]*)\]\s*\??\.\s*set\s*\(/g;
/** `{ secrets: alias` / `, secrets: alias` — a destructuring rename. */
const SECRETS_ALIAS = /[{,]\s*secrets\s*:\s*([A-Za-z_$][\w$]*)/g;

const escapeRegExp = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");

/**
 * The offset just past the `(` of every `ctx.secrets.set` call in `code`
 * (`source` masked), however the chain is spelled: `secrets.set(`,
 * `secrets?.set(`, `["secrets"].set(`, and a destructured alias
 * (`const { secrets: s } = ctx; s.set(`). The server's publish gate reads the
 * same forms (`custom_apps_secrets::shared_env::scan`).
 */
function secretsSetCalls(source: string, code: string): number[] {
  const calls = [...code.matchAll(SECRETS_SET)].map((m) => (m.index ?? 0) + m[0].length);
  for (const m of code.matchAll(BRACKET_SET)) {
    const open = (m.index ?? 0) + 1;
    const inner = source.slice(open, open + (m[1] ?? "").length).trim();
    if (literalString(inner) === "secrets") calls.push((m.index ?? 0) + m[0].length);
  }
  const aliases = new Set([...code.matchAll(SECRETS_ALIAS)].map((m) => m[1] as string));
  for (const alias of aliases) {
    const call = new RegExp(`(?<![\\w$.])${escapeRegExp(alias)}\\s*\\??\\.\\s*set\\s*\\(`, "g");
    for (const m of code.matchAll(call)) calls.push((m.index ?? 0) + m[0].length);
  }
  return calls.sort((a, b) => a - b);
}

const isObject = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value);

/** Each `….secrets.set("<literal>", …)` in `source`: the key and its line. */
export function secretsSetKeys(source: string): Array<{ key: string; line: number }> {
  const code = maskJs(source);
  const keys: Array<{ key: string; line: number }> = [];
  for (const open of secretsSetCalls(source, code)) {
    const first = splitArgs(code, open)?.[0];
    if (!first) continue;
    const key = literalString(source.slice(first.start, first.end).trim());
    if (key !== undefined) keys.push({ key, line: lineAt(source, open) });
  }
  return keys;
}

/** The keys the `env` block marks `shared: true`, and any malformed flag. */
function sharedKeys(manifest: Record<string, unknown>, issues: PlacementIssue[]): Set<string> {
  const shared = new Set<string>();
  const env = isObject(manifest.env) ? manifest.env : {};
  for (const [key, decl] of Object.entries(env)) {
    if (!isObject(decl) || decl.shared === undefined) continue;
    if (typeof decl.shared !== "boolean") {
      issues.push({
        level: "error",
        file: MANIFEST,
        path: `env.${key}.shared`,
        message: "must be a boolean"
      });
    } else if (decl.shared) {
      shared.add(key);
    }
  }
  return shared;
}

/** Every `shared` key that a function writes or names as a webhook secret. */
export function checkSharedEnv(
  appDir: string,
  manifest: Record<string, unknown>
): PlacementIssue[] {
  const issues: PlacementIssue[] = [];
  const shared = sharedKeys(manifest, issues);
  if (shared.size === 0) return issues;
  const functions = isObject(manifest.functions) ? manifest.functions : {};
  for (const [name, spec] of Object.entries(functions)) {
    if (!isObject(spec)) continue;
    const webhook = isObject(spec.webhook) ? spec.webhook : {};
    const secretVars =
      typeof webhook.secretVar === "string"
        ? webhook.secretVar.split(",").map((v) => v.trim())
        : [];
    for (const key of secretVars.filter((v) => shared.has(v))) {
      issues.push({
        level: "error",
        file: MANIFEST,
        path: `env.${key}.shared`,
        message:
          `\`${key}\` is function \`${name}\`'s webhook.secretVar; each environment verifies ` +
          "with its own value, so it cannot be shared — set a staging value instead"
      });
    }
    let source: string;
    try {
      source = readFileSync(join(appDir, functionEntry(manifest as PublishManifest, name)), "utf8");
    } catch {
      continue; // the placement check already warns that it could not read it
    }
    for (const { key, line } of secretsSetKeys(source).filter((k) => shared.has(k.key))) {
      issues.push({
        level: "error",
        file: MANIFEST,
        path: `env.${key}.shared`,
        message:
          `\`${key}\` is written by function \`${name}\` (ctx.secrets.set, line ${line}); a ` +
          "written key is set per environment, so it cannot be shared"
      });
    }
  }
  return issues;
}
