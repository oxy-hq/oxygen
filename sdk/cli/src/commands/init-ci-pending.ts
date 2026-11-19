/**
 * `oxyc init-ci --no-wait`: ask an org admin to approve the service account
 * and trust policy, print where, and EXIT. The next `init-ci` for the same
 * app, repository, workflow and environment collects the answer.
 *
 * FOR A CALLER THAT CANNOT WATCH A COMMAND THAT IS STILL RUNNING — an agent in
 * a Claude Code remote session sees output only once a command ends, so an
 * `init-ci` that prints a link and then waits ten minutes for someone to open
 * it is one nobody is shown. Split in two, the agent relays the link and ends
 * its turn; a re-run checks once and finishes, or says it is still waiting.
 *
 * NOTHING IS LOST IF NOBODY COLLECTS. A trust policy's approval writes the
 * rows itself, under the approver's session; collecting only reports them, so
 * the account's id can go into the workflow. A run that may read the rows
 * finds them whether or not this request was ever collected.
 *
 * The request lives in `init-ci-pending.json`, beside the credentials file and
 * owner-only like it (the device code and the PKCE verifier together collect
 * the answer). An entry goes once the deployment has decided: collected,
 * refused, lapsed or spent.
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { credentialsPath, hostKey, writePrivateJson } from "../auth/credentials.js";
import {
  announce,
  type DeviceRequest,
  deviceStart,
  deviceUrl,
  pollUntilDecided
} from "../auth/device.js";
import type { BrowserPurpose, CodeRefused, LoginOptions, MintBody } from "../auth/login.js";
import { createPkce } from "../auth/pkce.js";
import { CliError } from "../util/errors.js";

/** One started, uncollected approval. */
export interface PendingApproval {
  device_code: string;
  user_code: string;
  verifier: string;
  interval: number;
  /** RFC 3339: when the deployment forgets the request. */
  expires_at: string;
}

type Store = Record<string, PendingApproval>;

/** Beside the credentials file, so `OXY_CREDENTIALS_PATH` moves both. */
export function initCiPendingPath(): string {
  return join(dirname(credentialsPath()), "init-ci-pending.json");
}

function read(): Store {
  try {
    const parsed: unknown = JSON.parse(readFileSync(initCiPendingPath(), "utf8"));
    return parsed && typeof parsed === "object" && !Array.isArray(parsed) ? (parsed as Store) : {};
  } catch {
    return {};
  }
}

function write(store: Store): void {
  writePrivateJson(initCiPendingPath(), store, "init-ci-pending");
}

/** What one approval is for: a run of another plan never collects it. */
export function pendingKey(
  target: string,
  plan: {
    org: string;
    app: string;
    accountOrg: string;
    accountName: string;
    repository?: string;
    workflowPath: string;
    environment: string;
  }
): string {
  return [
    hostKey(target),
    `${plan.org}/${plan.app}`,
    `${plan.accountOrg}/${plan.accountName}`,
    plan.repository ?? "",
    plan.workflowPath,
    plan.environment
  ].join(" ");
}

/** The recorded request for `key`, unless it has lapsed. */
export function loadApproval(key: string, now: number = Date.now()): PendingApproval | undefined {
  const entry = read()[key];
  if (!entry || typeof entry.device_code !== "string" || typeof entry.verifier !== "string") {
    return undefined;
  }
  const expires = Date.parse(entry.expires_at);
  return Number.isFinite(expires) && expires > now ? entry : undefined;
}

export function clearApproval(key: string): void {
  const store = read();
  if (!(key in store)) return;
  delete store[key];
  write(store);
}

/** A request that is still waiting for someone to decide it. */
export interface Waiting {
  url: string;
  userCode: string;
  expiresAt: string;
}

/**
 * Start a request, record it, say where to approve it — and return without
 * waiting. Recorded BEFORE it is announced: on a read-only HOME the write
 * fails, and nobody is sent to approve a request this machine could not collect.
 */
export async function startApproval(
  target: string,
  key: string,
  opts: LoginOptions,
  ask: { query: string; purpose: BrowserPurpose }
): Promise<Waiting | undefined> {
  const pkce = createPkce();
  let expiresAt = "";
  const started = await deviceStart(target, opts, pkce, ask, (request) => {
    expiresAt = new Date(Date.now() + request.expires_in * 1000).toISOString();
    const store = read();
    store[key] = {
      device_code: request.device_code,
      user_code: request.user_code,
      verifier: pkce.verifier,
      interval: request.interval,
      expires_at: expiresAt
    };
    write(store);
  });
  if (!started) return undefined;
  return { url: deviceUrl(target, started.user_code), userCode: started.user_code, expiresAt };
}

/** Thrown by the one-poll `sleep`: the request is still waiting. */
class StillWaiting extends Error {}

/**
 * Collect a recorded request. `once`: poll a single time and answer
 * `undefined` while nobody has decided (a `--no-wait` re-run); otherwise wait
 * for the decision, as a plain re-run does.
 *
 * The entry goes once the deployment has decided — collected, refused, lapsed
 * or spent. A network failure keeps it: the request may still be collectable.
 */
export async function collectApproval(
  target: string,
  key: string,
  pending: PendingApproval,
  opts: LoginOptions,
  wording: { refused: CodeRefused; purpose: BrowserPurpose },
  once: boolean,
  now: number = Date.now()
): Promise<MintBody | undefined> {
  const request: DeviceRequest = {
    device_code: pending.device_code,
    user_code: pending.user_code,
    interval: pending.interval > 0 ? pending.interval : 5,
    expires_in: Math.max(0, Math.round((Date.parse(pending.expires_at) - now) / 1000))
  };
  const sleep = once
    ? async () => {
        throw new StillWaiting();
      }
    : (opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms))));
  if (!once) announce(target, request, wording.purpose, opts.approver);
  try {
    const body = await pollUntilDecided<MintBody>(
      target,
      pending.verifier,
      request,
      { ...wording, collect: (answer: MintBody) => answer },
      sleep,
      0
    );
    clearApproval(key);
    return body;
  } catch (cause) {
    if (cause instanceof StillWaiting) return undefined;
    if (cause instanceof CliError && cause.serverCode) clearApproval(key);
    throw cause;
  }
}
