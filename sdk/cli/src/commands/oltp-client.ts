/**
 * The wire half of `oxyc oltp`: one request helper, the refusals an operator can
 * act on, and the confirmation prompt — shared by the store (`oltp.ts`) and its
 * staging branch (`oltp-branch.ts`).
 */

import { createInterface } from "node:readline/promises";

import { type ApiResponse, parseJson, request } from "../api/request.js";
import type { Context } from "../context/resolve.js";
import { CliError, ExitCode, exitCodeForStatus } from "../util/errors.js";

/** Why the server refused, in the terms an operator acts on. */
function refusalReason(status: number): string | undefined {
  switch (status) {
    case 400:
      return "the server rejected the request as malformed";
    case 401:
      return "not authenticated for this target";
    case 403:
      return (
        "the OLTP console needs platform staff standing that may operate it — and an active " +
        "`oxyc assume` session closes /admin"
      );
    case 404:
      return "no such org here — or one outside your staff grant's scope, which answers 404 on purpose";
    case 409:
      return "the provisioner refused a state an operator has to resolve";
    case 503:
      return "this deployment has no OLTP provider configured — not an Oxy fault";
    default:
      return undefined;
  }
}

function oltpError(method: string, path: string, response: ApiResponse): CliError {
  const body = response.body.trim();
  const detail = [refusalReason(response.status), body && body !== "{}" ? `server: ${body}` : ""]
    .filter((line): line is string => Boolean(line))
    .join("\n");
  return new CliError(`${method} ${path} failed (${response.status})`, {
    code: exitCodeForStatus(response.status),
    detail: detail || undefined,
    remedy: response.status === 401 ? "oxyc login --env <env>" : undefined
  });
}

/** A 2xx JSON body, or the error its status deserves. */
export async function call<T>(
  ctx: Context,
  method: string,
  path: string,
  opts: { body?: unknown; timeoutMs?: number } = {}
): Promise<T> {
  const response = await request({
    target: ctx.target(),
    path,
    method,
    bearer: ctx.bearer(),
    body: opts.body === undefined ? undefined : JSON.stringify(opts.body),
    timeoutMs: opts.timeoutMs
  });
  if (response.status < 200 || response.status >= 300) throw oltpError(method, path, response);
  const parsed = parseJson(response.body);
  if (parsed === undefined || parsed === null) {
    throw new CliError(`${method} ${path} did not return JSON`, {
      code: ExitCode.FAILURE,
      detail: response.body.trim().slice(0, 500) || undefined,
      hint: "the deployment may predate this route — `oxyc routes oltp` lists what it mounts"
    });
  }
  return parsed as T;
}

export const orgPath = (orgId: string) => `/api/admin/orgs/${encodeURIComponent(orgId)}/oltp`;

/**
 * Provisioning creates a project at the provider and its roles, which is slower
 * than a read. The default two-minute client timeout would abandon a call the
 * server is still completing; retrying is safe, but the timeout reads as failure.
 */
export const PROVISION_TIMEOUT_MS = 5 * 60_000;

/**
 * Never assumes yes. With no terminal to ask, an unconfirmed action refuses
 * rather than proceeding or hanging on a prompt nobody can answer.
 */
export async function confirm(
  question: string,
  refusal: { verb: string; why: string; declined: string }
): Promise<void> {
  if (!process.stdin.isTTY) {
    throw new CliError(`${refusal.verb} needs --yes when stdin is not a terminal`, {
      code: ExitCode.REFUSED,
      detail: refusal.why,
      remedy: "re-run with --yes once the plan above is what you want"
    });
  }
  const prompt = createInterface({ input: process.stdin, output: process.stderr });
  let answer: string;
  try {
    answer = (await prompt.question(`${question} [y/N] `)).trim().toLowerCase();
  } finally {
    prompt.close();
  }
  if (answer === "y" || answer === "yes") return;
  throw new CliError(refusal.declined, { code: ExitCode.REFUSED });
}
