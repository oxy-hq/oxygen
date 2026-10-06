/**
 * `oxyc env secret` — a SANDBOX's own secrets: list the keys, set one, delete
 * one. `/api/customer-apps/{id}/secrets` with `environment` naming the sandbox
 * (`crates/app/src/server/api/custom_apps_secrets/`).
 *
 * SANDBOXES ONLY, for every credential. The route also holds staging's and
 * production's secrets; a verb an agent drives unattended does not reach them,
 * the same way `oxyc publish --app-env` accepts only a `dev-<handle>` name.
 *
 * NEVER A VALUE. The list is keys and flags, and there is no reveal verb: a
 * value in an agent's context is a value an injected instruction can print.
 *
 * These exist because a sandbox agent token is refused `oxyc api`, which is
 * how a sandbox's secret was set before. `oxyc mcp` serves the same three
 * functions as `oxy_env_secret_list` / `_set` / `_delete`.
 */

import { parseJson, request } from "../api/request.js";
import { requireSandboxName } from "../apps/environment.js";
import { ensureOk, resolveApp, staffCreds } from "../apps/resolve.js";
import type { Context } from "../context/resolve.js";
import { out } from "../ui/tty.js";
import { usageError } from "../util/errors.js";

/** One row of the list. The server's `AppSecretEntry`, printed verbatim. */
export interface SecretEntry {
  key: string;
  is_set: boolean;
  declared?: boolean;
  required?: boolean;
  /** The sandbox holds no value of its own and reads staging's. */
  inherits_staging?: boolean;
  description?: string | null;
  updated_at?: string | null;
}

/** The server's `AppSecretsResponse`. Fields this tool does not read are kept. */
export interface SecretsView {
  environment?: string;
  entries?: SecretEntry[];
  /** Declared `required` keys with no value anywhere this sandbox reads. */
  missing_required?: number;
}

export interface SecretChange {
  key: string;
  environment: string;
  status: "set" | "deleted";
}

function secretsPath(appId: string, key?: string): string {
  const base = `/api/customer-apps/${appId}/secrets`;
  return key === undefined ? base : `${base}/${encodeURIComponent(key)}`;
}

/** A key is one path segment. The server holds it to its own name grammar. */
function requireKey(raw: string): string {
  const key = raw.trim();
  if (!key || /[\s/]/.test(key)) {
    throw usageError(
      `${JSON.stringify(raw)} is not a secret key`,
      "a key is one name, e.g. STRIPE_SECRET_KEY — no spaces, no slashes"
    );
  }
  return key;
}

export async function secretList(ctx: Context, app: string, appEnv: string): Promise<SecretsView> {
  const sandbox = requireSandboxName(appEnv);
  const creds = staffCreds(ctx);
  const resolved = await resolveApp(creds, app);
  const response = await request({
    target: creds.target,
    path: `${secretsPath(resolved.appId)}?environment=${encodeURIComponent(sandbox)}`,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  return (parseJson(response.body) as SecretsView | undefined) ?? {};
}

export async function secretSet(
  ctx: Context,
  app: string,
  appEnv: string,
  rawKey: string,
  value: string
): Promise<SecretChange> {
  const sandbox = requireSandboxName(appEnv);
  const key = requireKey(rawKey);
  if (!value.trim()) throw usageError(`the value for ${key} is empty`);
  const creds = staffCreds(ctx);
  const resolved = await resolveApp(creds, app);
  const response = await request({
    target: creds.target,
    path: secretsPath(resolved.appId),
    method: "POST",
    body: JSON.stringify({ key, value, environment: sandbox }),
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  return { key, environment: sandbox, status: "set" };
}

export async function secretDelete(
  ctx: Context,
  app: string,
  appEnv: string,
  rawKey: string
): Promise<SecretChange> {
  const sandbox = requireSandboxName(appEnv);
  const key = requireKey(rawKey);
  const creds = staffCreds(ctx);
  const resolved = await resolveApp(creds, app);
  const response = await request({
    target: creds.target,
    path: `${secretsPath(resolved.appId, key)}?environment=${encodeURIComponent(sandbox)}`,
    method: "DELETE",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  return { key, environment: sandbox, status: "deleted" };
}

/** `--app-env`, required: these verbs have no production default. */
function requireAppEnv(appEnv: string | undefined): string {
  if (appEnv === undefined) {
    throw usageError(
      "oxyc env secret needs --app-env dev-<handle>",
      "it reads and writes one sandbox's secrets, never staging's or production's"
    );
  }
  return appEnv;
}

/**
 * The value, from exactly one of `--value` and `--value-env`. The second
 * keeps it out of the argument list, where a process listing can read it.
 */
export function secretValue(opts: { value?: string; valueEnv?: string }): string {
  if ((opts.value === undefined) === (opts.valueEnv === undefined)) {
    throw usageError(
      "pass exactly one of --value <text> and --value-env <VAR>",
      "--value-env reads it from an environment variable, keeping it out of the command line"
    );
  }
  if (opts.value !== undefined) return opts.value;
  const fromEnv = process.env[opts.valueEnv as string];
  if (!fromEnv) throw usageError(`--value-env ${opts.valueEnv} is not set`);
  return fromEnv;
}

export async function runEnvSecretList(
  ctx: Context,
  app: string,
  opts: { appEnv?: string; json: boolean }
): Promise<void> {
  const view = await secretList(ctx, app, requireAppEnv(opts.appEnv));
  if (opts.json) {
    process.stdout.write(`${JSON.stringify(view)}\n`);
    return;
  }
  const entries = view.entries ?? [];
  if (entries.length === 0) {
    process.stderr.write("no secrets declared or set\n");
    return;
  }
  for (const entry of entries) {
    const state = entry.is_set ? "set" : entry.inherits_staging ? "staging's" : "unset";
    const required = entry.required ? " required" : "";
    process.stdout.write(`${entry.key.padEnd(32)} ${state}${required}\n`);
  }
}

export async function runEnvSecretSet(
  ctx: Context,
  app: string,
  key: string,
  opts: { appEnv?: string; value?: string; valueEnv?: string; json: boolean }
): Promise<void> {
  const appEnv = requireAppEnv(opts.appEnv);
  const change = await secretSet(ctx, app, appEnv, key, secretValue(opts));
  if (opts.json) process.stdout.write(`${JSON.stringify(change)}\n`);
  else process.stdout.write(`${out.green("set")} ${change.key} in ${change.environment}\n`);
}

export async function runEnvSecretDelete(
  ctx: Context,
  app: string,
  key: string,
  opts: { appEnv?: string; json: boolean }
): Promise<void> {
  const change = await secretDelete(ctx, app, requireAppEnv(opts.appEnv), key);
  if (opts.json) process.stdout.write(`${JSON.stringify(change)}\n`);
  else process.stdout.write(`${out.yellow("deleted")} ${change.key} from ${change.environment}\n`);
}
