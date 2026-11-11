/**
 * `oxyc login --no-wait`: start a device-code login, print where to approve
 * it, and EXIT. The next `oxyc login` for the same deployment collects it.
 *
 * FOR A CALLER THAT CANNOT WATCH A COMMAND THAT IS STILL RUNNING — an agent
 * in a Claude Code remote session sees a command's output only once it ends,
 * so a login that prints a URL and then waits ten minutes for someone to open
 * it is a login nobody is shown. Split in two, the first command hands back
 * the URL (alone on stdout, so `$(oxyc login --no-wait)` is the URL) and the
 * second, run once the person has approved, finishes in one poll.
 *
 * The request lives in `login-pending.json`, beside the credentials file and
 * owner-only like it: the device code and the PKCE verifier together collect
 * the token once someone approves, so the file is as sensitive as the token
 * for those ten minutes. It is removed when the request is collected, refused
 * or lapses; a stale entry is ignored by its own `expires_at`.
 */

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import * as log from "../ui/log.js";
import { CliError, ExitCode } from "../util/errors.js";
import { credentialsPath, hostKey, writePrivateJson } from "./credentials.js";
import {
  announce,
  announceWaiting,
  type DeviceRequest,
  defaultSleep,
  deviceStart,
  deviceUrl,
  pollUntilDecided
} from "./device.js";
import { LOGIN_CODE_REFUSED, LOGIN_PURPOSE, type LoginOptions, type Minted } from "./login.js";
import { createPkce } from "./pkce.js";

/** One started, uncollected request. */
interface PendingLogin {
  device_code: string;
  user_code: string;
  verifier: string;
  interval: number;
  /** RFC 3339: when the deployment forgets the request. */
  expires_at: string;
}

type PendingStore = Record<string, PendingLogin>;

/** Beside the credentials file, so `OXY_CREDENTIALS_PATH` moves both. */
export function pendingPath(): string {
  return join(dirname(credentialsPath()), "login-pending.json");
}

function readPending(): PendingStore {
  try {
    const parsed: unknown = JSON.parse(readFileSync(pendingPath(), "utf8"));
    return parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as PendingStore)
      : {};
  } catch {
    return {};
  }
}

function writePending(store: PendingStore): void {
  writePrivateJson(pendingPath(), store, "login-pending");
}

function savePending(target: string, entry: PendingLogin): void {
  const store = readPending();
  store[hostKey(target)] = entry;
  writePending(store);
}

/** Forget `target`'s request. A no-op, with no write, when there is none. */
export function clearPending(target: string): void {
  const store = readPending();
  const key = hostKey(target);
  if (!(key in store)) return;
  delete store[key];
  writePending(store);
}

/** `target`'s request, if one is recorded and has not lapsed. */
export function loadPending(target: string, now: number = Date.now()): PendingLogin | undefined {
  const entry = readPending()[hostKey(target)];
  if (!entry || typeof entry.device_code !== "string" || typeof entry.verifier !== "string") {
    return undefined;
  }
  const expires = Date.parse(entry.expires_at);
  return Number.isFinite(expires) && expires > now ? entry : undefined;
}

/**
 * Start a request, record it, say where to approve it, and return the URL.
 * `finish` is the command that collects it, as the person should type it.
 */
export async function startPendingLogin(
  target: string,
  opts: LoginOptions,
  finish: string
): Promise<string> {
  const pkce = createPkce();
  // Recorded BEFORE the URL is printed: on a read-only HOME the write fails
  // and nobody is sent to approve a request this machine could never collect.
  const started = await deviceStart(target, opts, pkce, {}, (request) =>
    savePending(target, {
      device_code: request.device_code,
      user_code: request.user_code,
      verifier: pkce.verifier,
      interval: request.interval,
      expires_at: new Date(Date.now() + request.expires_in * 1000).toISOString()
    })
  );
  if (!started) {
    throw new CliError(`${target} cannot approve a login by device code`, {
      code: ExitCode.UNAVAILABLE,
      detail: "POST /api/auth/cli/device answered 404: the deployment predates `--no-browser`.",
      hint: "run `oxyc login` where a browser can open, or set OXY_TOKEN"
    });
  }
  log.say(
    `Once it is approved, run \`${finish}\` to finish (within ${Math.round(started.expires_in / 60)} min).`
  );
  return deviceUrl(target, started.user_code);
}

/**
 * Collect the request `--no-wait` started, waiting for it if nobody has
 * approved it yet. The URL and code are printed again, so a person who lost
 * the first output can still approve.
 *
 * The entry goes once the deployment has decided — collected, refused, lapsed
 * or spent: none of those can be collected again. A network failure keeps it,
 * since the request may still be waiting to be collected.
 */
export async function resumePendingLogin(
  target: string,
  pending: PendingLogin,
  opts: LoginOptions,
  now: number = Date.now()
): Promise<Minted> {
  const request: DeviceRequest = {
    device_code: pending.device_code,
    user_code: pending.user_code,
    interval: pending.interval > 0 ? pending.interval : 5,
    expires_in: Math.max(0, Math.round((Date.parse(pending.expires_at) - now) / 1000))
  };
  log.info(`Collecting the login request \`oxyc login --no-wait\` started.`);
  announce(target, request, LOGIN_PURPOSE, undefined);
  announceWaiting(request.expires_in);
  let minted: Minted;
  try {
    minted = await pollUntilDecided(
      target,
      pending.verifier,
      request,
      { refused: LOGIN_CODE_REFUSED, purpose: LOGIN_PURPOSE },
      opts.sleep ?? defaultSleep,
      0
    );
  } catch (cause) {
    if (cause instanceof CliError && cause.serverCode) clearPending(target);
    throw cause;
  }
  clearPending(target);
  return minted;
}
