/**
 * The browser loopback login, over PKCE.
 *
 * Bind an ephemeral `127.0.0.1` port, send the browser to
 * `<target>/cli-auth?port&state&code_challenge&hostname`, and catch what the
 * web app hands back to `/callback`:
 *
 *   `?code=`    a one-time code. Traded, with the PKCE verifier that never left
 *               this process, at `POST /api/auth/cli/exchange` for an
 *               `oxy_pat_` personal access token named `oxyc on <hostname>` —
 *               revocable, ninety days, and replacing the host's previous one.
 *   `?token=`   the session JWT itself — what a deployment that predates the
 *               exchange hands back, having ignored the two parameters it does
 *               not know. Cached exactly as it always was.
 *
 * BOTH ARE ACCEPTED, and which one arrives is the deployment's decision, not a
 * flag: a new CLI against an old deployment logs in the old way, and an old
 * CLI against a new deployment (no `code_challenge` in its URL) is handed a
 * token as before. Neither needs the other upgraded first.
 *
 * The callback path, the `state` check and the five-minute deadline are the
 * Rust `login.rs` protocol (since deleted), unchanged. They are shared with a
 * page in another package, so drifting from them is a silent "login hangs
 * forever", not a compile error.
 */

import { spawn } from "node:child_process";
import { randomUUID } from "node:crypto";
import { createServer, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { hostname as osHostname } from "node:os";
import * as log from "../ui/log.js";
import { err } from "../ui/tty.js";
import { CliError, ExitCode, exitCodeForStatus } from "../util/errors.js";
import { type HostCredential, saveCredential } from "./credentials.js";
import { createPkce, type Pkce } from "./pkce.js";

/** What `/api/user` gives back. Only the two fields the login flow reports. */
interface UserResponse {
  email?: string;
  is_app_admin?: boolean;
}

export interface LoginOptions {
  /** Open the auth URL. Tests inject one; the default asks the OS. */
  open?: (url: string) => void;
  /** Names the token `oxyc on <hostname>`. Defaults to this machine's. */
  hostname?: string;
}

/** What the loopback caught. */
type Callback = { kind: "code"; code: string } | { kind: "token"; token: string };

/** A token ready to cache, with what the exchange said about it. */
interface Minted {
  token: string;
  tokenId?: string;
  expiresAt?: string;
}

/** The tab the user is left looking at. */
const SUCCESS_HTML =
  "<!doctype html><meta charset=utf-8><title>oxyc login</title>" +
  '<body style="font-family:system-ui;padding:3rem;text-align:center">' +
  "<h2>Logged in to oxy ✓</h2><p>You can close this tab and return to your terminal.</p>";

/** The Rust waits 5 minutes. Long enough for an SSO detour with an MFA prompt. */
const LOGIN_TIMEOUT_MS = 300_000;

function base(target: string): string {
  return target.replace(/\/+$/, "");
}

/**
 * Run the loopback flow against one target and cache the result.
 *
 * Returns the token as well as storing it, so a caller chaining straight into
 * an assume-role session does not have to read the file it just wrote.
 */
export async function login(
  target: string,
  opts: LoginOptions = {}
): Promise<{ token: string; user: HostCredential }> {
  const minted = await obtainToken(target, opts);
  const user = await fetchUser(target, minted.token);

  const credential: HostCredential = {
    token: minted.token,
    email: user.email ?? "",
    is_app_admin: Boolean(user.is_app_admin),
    // Present only when the exchange supplied them, so a session-token login
    // writes the same three fields it always did.
    ...(minted.tokenId ? { token_id: minted.tokenId } : {}),
    ...(minted.expiresAt ? { expires_at: minted.expiresAt } : {})
  };
  saveCredential(target, credential);
  return { token: minted.token, user: credential };
}

/**
 * One browser round with PKCE, and a second without it only when the
 * deployment issued a code it then cannot exchange.
 *
 * That second case is a deployment mid-rollout — a web app new enough to issue
 * codes in front of a server that has no exchange route yet. Rare, but the
 * alternative is a login that cannot succeed until someone finishes a deploy.
 */
async function obtainToken(target: string, opts: LoginOptions): Promise<Minted> {
  const pkce = createPkce();
  const first = await browserRound(target, opts, pkce);
  if (first.kind === "token") {
    log.info(
      "this deployment handed back a session token — it predates revocable CLI tokens. Cached as before."
    );
    return { token: first.token };
  }

  const exchanged = await exchangeCliCode(target, first.code, pkce.verifier);
  if (exchanged) return exchanged;

  log.warn(
    `${target} issued a login code but has no exchange route (POST /api/auth/cli/exchange answered 404)`
  );
  log.hint("falling back to the session-token login — the browser will open once more");
  const second = await browserRound(target, opts, undefined);
  if (second.kind !== "token") {
    throw new CliError("the deployment issued a login code that it cannot exchange", {
      code: ExitCode.UNAVAILABLE,
      hint: "its web app and server are on different versions — retry once the deploy has finished, or set OXY_TOKEN"
    });
  }
  return { token: second.token };
}

/** Open the browser at `/cli-auth` and wait for the callback. */
async function browserRound(
  target: string,
  opts: LoginOptions,
  pkce: Pkce | undefined
): Promise<Callback> {
  const state = randomUUID();
  const { port, waitForCallback, close } = await startLoopback(state);
  try {
    const authUrl = cliAuthUrl(target, port, state, pkce, opts.hostname ?? osHostname());
    log.info(`Opening ${authUrl} in your browser to log in…`);
    log.info("If it doesn't open automatically, paste that URL into your browser.");
    (opts.open ?? openBrowser)(authUrl);
    return await waitForCallback;
  } finally {
    close();
  }
}

/**
 * The page URL. `port` and `state` are the original protocol; the challenge
 * and the hostname are what a deployment with the exchange reads to issue a
 * code instead of a token. Without a `pkce` this is the original URL exactly.
 */
export function cliAuthUrl(
  target: string,
  port: number,
  state: string,
  pkce: Pkce | undefined,
  hostname: string
): string {
  const url = `${base(target)}/cli-auth?port=${port}&state=${encodeURIComponent(state)}`;
  if (!pkce) return url;
  return `${url}&code_challenge=${encodeURIComponent(pkce.challenge)}&hostname=${encodeURIComponent(hostname)}`;
}

/**
 * Trade the one-time code for a token.
 *
 * `undefined` means the route is not there (404) — the caller's cue to fall
 * back. Every other failure throws: a rejected code is not something a second
 * attempt at the same exchange can fix.
 */
export async function exchangeCliCode(
  target: string,
  code: string,
  verifier: string
): Promise<Minted | undefined> {
  const url = `${base(target)}/api/auth/cli/exchange`;
  let response: Response;
  try {
    response = await fetch(url, {
      method: "POST",
      headers: { "content-type": "application/json", accept: "application/json" },
      body: JSON.stringify({ code, code_verifier: verifier }),
      signal: AbortSignal.timeout(30_000)
    });
  } catch (cause) {
    throw new CliError(`POST ${url} failed: ${(cause as Error).message}`, {
      code: ExitCode.UNAVAILABLE
    });
  }
  if (response.status === 404) return undefined;

  const text = await response.text();
  let body: { token?: { id?: string; expires_at?: string | null }; secret?: string; code?: string };
  try {
    body = JSON.parse(text);
  } catch {
    body = {};
  }
  if (!response.ok) {
    if (body.code === "invalid_code") {
      throw new CliError("the login code was rejected", {
        code: ExitCode.AUTH,
        detail: "a code is single-use and lives five minutes.",
        hint: "run `oxyc login` again"
      });
    }
    throw new CliError(`the login exchange failed (${response.status})`, {
      code: exitCodeForStatus(response.status),
      detail: text.slice(0, 2000) || undefined
    });
  }
  if (!body.secret) {
    throw new CliError("the login exchange returned no token", { code: ExitCode.UNAVAILABLE });
  }
  return {
    token: body.secret,
    tokenId: body.token?.id,
    expiresAt: body.token?.expires_at ?? undefined
  };
}

/**
 * The loopback listener.
 *
 * It keeps accepting until it sees a callback whose `state` matches, rather
 * than resolving on the first request: a browser will cheerfully send a
 * `/favicon.ico` alongside the redirect, and treating that as the callback
 * would fail a login that was actually about to succeed.
 */
async function startLoopback(expectedState: string): Promise<{
  port: number;
  waitForCallback: Promise<Callback>;
  close: () => void;
}> {
  let resolveCallback!: (callback: Callback) => void;
  let rejectCallback!: (reason: Error) => void;
  const waitForCallback = new Promise<Callback>((resolve, reject) => {
    resolveCallback = resolve;
    rejectCallback = reject;
  });
  // The deadline can fire while nothing is awaiting the promise yet.
  waitForCallback.catch(() => {});

  const server = createServer((req, res) => {
    // `req.url` is a path+query, so it needs a base to parse. The base is
    // discarded — only the path and the query matter.
    const parsed = new URL(req.url ?? "/", "http://localhost");
    if (parsed.pathname !== "/callback") {
      plain(res, 404, "Not found");
      return;
    }
    if (parsed.searchParams.get("state") !== expectedState) {
      // A mismatch means this callback belongs to some other login attempt —
      // or to something that guessed the port. Refuse and keep listening.
      plain(res, 400, "State mismatch — please retry `oxyc login`.");
      return;
    }
    // `code` first: a page that sent both would be a page that can do the
    // exchange, and the code is the one that does not put a credential in a URL.
    const code = parsed.searchParams.get("code");
    const token = parsed.searchParams.get("token");
    if (!code && !token) {
      plain(res, 400, "No code or token in callback.");
      return;
    }
    res.writeHead(200, {
      "Content-Type": "text/html; charset=utf-8",
      Connection: "close"
    });
    res.end(SUCCESS_HTML);
    resolveCallback(code ? { kind: "code", code } : { kind: "token", token: token as string });
  });

  const timer = setTimeout(() => {
    rejectCallback(
      new CliError("timed out waiting for the browser to complete login (5 min)", {
        code: ExitCode.UNAVAILABLE,
        hint: "oxyc login --env <env>   — and complete the browser flow"
      })
    );
  }, LOGIN_TIMEOUT_MS);
  // Do not hold the process open on the timer alone; the promise decides.
  timer.unref?.();

  await new Promise<void>((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });

  const address = server.address() as AddressInfo | null;
  if (!address) {
    server.close();
    throw new CliError("could not bind loopback port", { code: ExitCode.UNAVAILABLE });
  }

  return {
    port: address.port,
    waitForCallback,
    close: () => {
      clearTimeout(timer);
      server.close();
    }
  };
}

function plain(res: ServerResponse, status: number, body: string): void {
  res.writeHead(status, {
    "Content-Type": "text/plain; charset=utf-8",
    Connection: "close"
  });
  res.end(body);
}

/**
 * Confirm the token and find out who it belongs to.
 *
 * Not merely informational: it is the difference between "we captured a
 * string" and "the server accepts it". A token cached without this check
 * fails later, at some unrelated command, with a 401 nobody connects back to
 * the login.
 */
async function fetchUser(target: string, token: string): Promise<UserResponse> {
  const url = `${base(target)}/api/user`;
  let response: Response;
  try {
    response = await fetch(url, {
      headers: { Authorization: `Bearer ${token}` },
      signal: AbortSignal.timeout(30_000)
    });
  } catch (cause) {
    throw new CliError(`GET ${url} failed: ${(cause as Error).message}`, {
      code: ExitCode.UNAVAILABLE
    });
  }
  if (!response.ok) {
    throw new CliError(`login token rejected by ${url} (${response.status})`, {
      code: ExitCode.AUTH
    });
  }
  const body = (await response.json()) as UserResponse | null;
  if (!body) {
    throw new CliError("login token did not resolve to a user (got null)", {
      code: ExitCode.AUTH
    });
  }
  return body;
}

/** Best-effort browser open. A failure is fine — the URL was already printed. */
export function openBrowser(url: string): void {
  const [bin, args] =
    process.platform === "darwin"
      ? (["open", [url]] as const)
      : process.platform === "win32"
        ? (["cmd", ["/C", "start", "", url]] as const)
        : (["xdg-open", [url]] as const);
  try {
    const child = spawn(bin, [...args], { stdio: "ignore", detached: true });
    // A machine with no opener emits `error` asynchronously; unhandled, that
    // is an uncaught exception rather than the shrug it should be.
    child.on("error", () => {});
    child.unref();
  } catch {
    // The URL is on stderr already; a machine with no browser is a valid
    // place to run this, and failing the login over it would be wrong.
  }
}

/** The line `login` prints about publish rights, shared with `whoami`. */
export function adminStatusLine(credential: HostCredential): string {
  return credential.is_app_admin
    ? err.green("Global admin: yes — you can publish.")
    : err.yellow(
        "Global admin: no — you can't publish yet. Ask #platform to add you to OXY_GLOBAL_ADMINS."
      );
}
