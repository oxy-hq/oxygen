/**
 * `oxyc logs` — persisted `ctx.log()` / `console.*` output of an app's Oxy
 * Functions: `GET /api/customer-apps/{org}/{app}/logs`
 * (`crates/app/src/server/api/custom_apps_logs.rs`). ClickHouse-backed, so a
 * deployment with observability capture off answers `501` — not an error in
 * the usual sense, which is why it maps to the retryable exit code rather
 * than a hard failure (it may simply not be configured on THIS deployment).
 */

import { parseJson, request } from "../api/request.js";
import { parseAppEnv } from "../apps/environment.js";
import { ensureOk, resolveApp, staffCreds } from "../apps/resolve.js";
import { requireOwnSandbox } from "../apps/sandbox-token.js";
import { isSandboxAgentToken } from "../auth/token-kind.js";
import type { Context } from "../context/resolve.js";

export interface LogLine {
  timestamp: string;
  build_id: string;
  invocation_id: string;
  request_id: string;
  function_name: string;
  mode: string;
  level: string;
  seq: number;
  message: string;
  trace_id: string;
  /** New with sandboxes: which environment this line was written from. */
  environment?: string;
}

export interface LogsOptions {
  appEnv?: string;
  invocation?: string;
  request?: string;
  hours?: number;
  limit?: number;
}

export async function fetchLogs(ctx: Context, app: string, opts: LogsOptions): Promise<LogLine[]> {
  const appEnv = opts.appEnv !== undefined ? parseAppEnv(opts.appEnv) : undefined;
  const creds = staffCreds(ctx);
  // Before any request: with no environment this reads production's lines.
  if (isSandboxAgentToken(creds.bearer)) requireOwnSandbox("oxyc logs", appEnv);
  const resolved = await resolveApp(creds, app);

  const query = new URLSearchParams();
  if (appEnv !== undefined) query.set("environment", appEnv);
  if (opts.invocation !== undefined) query.set("invocation_id", opts.invocation);
  if (opts.request !== undefined) query.set("request_id", opts.request);
  if (opts.hours !== undefined) query.set("hours", String(opts.hours));
  if (opts.limit !== undefined) query.set("limit", String(opts.limit));
  const qs = query.toString();

  const response = await request({
    target: creds.target,
    path: `/api/customer-apps/${resolved.orgSlug}/${resolved.appSlug}/logs${qs ? `?${qs}` : ""}`,
    method: "GET",
    bearer: creds.bearer,
    headers: creds.headers
  });
  ensureOk(response, creds);
  const payload = parseJson(response.body) as { logs?: LogLine[] } | undefined;
  return payload?.logs ?? [];
}

export async function runLogs(
  ctx: Context,
  app: string,
  opts: LogsOptions & { json: boolean }
): Promise<void> {
  const logs = await fetchLogs(ctx, app, opts);
  if (opts.json) {
    process.stdout.write(`${JSON.stringify({ logs })}\n`);
    return;
  }
  printLogsTable(logs);
}

function printLogsTable(logs: LogLine[]): void {
  if (logs.length === 0) {
    process.stderr.write("no log lines in the window\n");
    return;
  }
  for (const line of logs) {
    const env = line.environment ? ` [${line.environment}]` : "";
    process.stdout.write(
      `${line.timestamp} ${line.level.padEnd(5)} ${line.function_name}${env} ${line.message}\n`
    );
  }
}
