/**
 * `oxyc tokens create --sandbox-agent` — mint a sandbox agent token
 * (`oxy_sbx_…`), and `oxyc tokens revoke --current` — end the one in use.
 *
 * THE MINT IS THE ONE COMMAND AN AGENT RUNS TO GET ITS OWN CREDENTIAL. It
 * holds no credential while it runs: minting is browser-session-only on the
 * server, so this opens the same PKCE loopback `oxyc login` uses, with four
 * more parameters saying what is being asked for. The agent's operator reads
 * the apps and the lifetime on the page and approves once. What comes back is
 * printed, once, as `export OXY_TOKEN=…` on stdout — everything else is on
 * stderr, so `eval "$(oxyc tokens create --sandbox-agent …)"` sets the variable
 * and nothing else.
 *
 * WHAT IT NEVER DOES: write the credentials file (the token is the agent's for
 * one task, not this machine's login), or revoke another token (`oxyc login`
 * replaces the host's previous token; this replaces nothing).
 */

import { hostname as osHostname } from "node:os";
import { forgetSandboxTokens } from "../apps/sandbox-token.js";
import {
  type BrowserPurpose,
  browserRound,
  exchangeCliCode,
  type LoginOptions
} from "../auth/login.js";
import { createPkce } from "../auth/pkce.js";
import { describeExpiry, revokeCallingToken } from "../auth/token-api.js";
import { isHeaderSafeSecret, isRevocable, isWellFormedSandboxToken } from "../auth/token-kind.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode, refusal, usageError } from "../util/errors.js";
import { printable } from "../util/printable.js";
import { type Mismatch, mismatch, unsupported } from "./tokens-sandbox-verify.js";

/** The server's limits (`token-options.sandbox_agent`), checked before the browser opens. */
export const SANDBOX_MINT = { maxApps: 5, minHours: 1, maxHours: 168, defaultHours: 8 } as const;

/** The server's cap on a token's name. */
const NAME_MAX_CHARS = 100;

/** One slug of `<org>/<app>`: nothing that could split the comma-joined list or the path. */
// ANCHORED AT BOTH ENDS, and it has to be: `sandboxMintQuery` writes these into
// the /cli-auth URL unescaped. A pattern that matched only the front would let
// `acme/store&hours=168` add a parameter of its own.
const SLUG_RE = /^[A-Za-z0-9][A-Za-z0-9._-]*$/;

export interface SandboxMintFlags {
  /** `--app`, once per occurrence. */
  apps: string[];
  /** `--hours`, as typed. */
  hours?: string;
  name?: string;
}

export interface SandboxMintRequest {
  /** `<org>/<app>`, distinct, in the order given. */
  apps: string[];
  hours: number;
  name: string;
}

const MINT_PURPOSE: BrowserPurpose = {
  opening: "approve a sandbox agent token",
  retry: "oxyc tokens create --sandbox-agent",
  done: "Sandbox agent token approved ✓",
  waitingFor: "approve the token"
};

function parseApps(raw: string[]): string[] {
  if (raw.length === 0) {
    throw usageError(
      "--sandbox-agent needs at least one --app <org>/<app>",
      "name the app the agent will work on — up to 5, one --app each"
    );
  }
  const seen = new Set<string>();
  for (const app of raw) {
    const parts = app.split("/");
    const [org = "", slug = ""] = parts;
    if (parts.length !== 2 || !SLUG_RE.test(org) || !SLUG_RE.test(slug)) {
      throw usageError(
        `--app ${JSON.stringify(app)} is not <org>/<app>`,
        "both are slugs, e.g. --app acme/store — an app UUID is not accepted here"
      );
    }
    const key = app.toLowerCase();
    if (seen.has(key)) throw usageError(`--app ${app} is named twice`);
    seen.add(key);
  }
  if (raw.length > SANDBOX_MINT.maxApps) {
    throw usageError(
      `a sandbox agent token reaches at most ${SANDBOX_MINT.maxApps} apps, and ${raw.length} were named`,
      "mint one token per task, for the apps that task touches"
    );
  }
  return raw;
}

/** `--hours`, for either token an agent mints: the server holds both to the same range. */
export function parseHours(raw: string | undefined): number {
  if (raw === undefined) return SANDBOX_MINT.defaultHours;
  const hours = Number(raw);
  if (!Number.isInteger(hours) || hours < SANDBOX_MINT.minHours || hours > SANDBOX_MINT.maxHours) {
    throw usageError(
      `--hours ${JSON.stringify(raw)} is not a whole number from ${SANDBOX_MINT.minHours} to ${SANDBOX_MINT.maxHours}`,
      `the default is ${SANDBOX_MINT.defaultHours}; 168 is seven days, and a token cannot be extended`
    );
  }
  return hours;
}

/** `--name`, or `unnamed` cut to the server's limit when none was given. */
export function parseName(raw: string | undefined, unnamed: string): string {
  if (raw === undefined) return unnamed.slice(0, NAME_MAX_CHARS);
  const name = raw.trim();
  if (!name) throw usageError("--name is empty");
  if (name.length > NAME_MAX_CHARS) {
    throw usageError(`--name is ${name.length} characters, and the limit is ${NAME_MAX_CHARS}`);
  }
  return name;
}

/**
 * Validate what was asked for. EVERY USAGE ERROR BEFORE ANY BROWSER OPENS: a
 * request the server would answer `400 invalid_sandbox_token` must not cost
 * the operator an approval click first.
 */
export function parseSandboxMint(flags: SandboxMintFlags, hostname: string): SandboxMintRequest {
  return {
    apps: parseApps(flags.apps),
    hours: parseHours(flags.hours),
    name: parseName(flags.name, `sandbox agent on ${hostname}`)
  };
}

/**
 * The four parameters added to `/cli-auth`, already encoded.
 *
 * `apps` keeps its `/` and `,` literal: they are the list's own separators,
 * legal in a query string, and what the page splits on. Each slug is already
 * held to `SLUG_RE`, so nothing in it needs escaping.
 */
export function sandboxMintQuery(ask: SandboxMintRequest): string {
  return [
    "kind=sandbox_agent",
    `apps=${ask.apps.join(",")}`,
    `hours=${ask.hours}`,
    `name=${encodeURIComponent(ask.name)}`
  ].join("&");
}

/**
 * Refuse what the deployment handed back. NOTHING IS PRINTED AND NOTHING IS
 * STORED: a token it minted is revoked here, best effort, with that token as
 * the bearer — nothing else will ever hold it, so nothing else could end it.
 *
 * Exit `8`, not `7`: this is not a blip to retry. It would have "worked", and
 * that is the problem — which is what a refusal is.
 *
 * Shared with `tokens create --agent`, which refuses what it is handed by the
 * same rule; `hint` is the one part that names the kind of token.
 */
export async function discard(
  target: string,
  problem: Mismatch,
  secret?: string,
  hint: string = SANDBOX_DISCARD_HINT
): Promise<never> {
  // Sent as a bearer only when it is made of credential characters: what a
  // deployment hands back in place of a token goes into no header either.
  const sendable = isHeaderSafeSecret(secret);
  const outcome = sendable ? await revokeCallingToken(target, secret) : undefined;
  const kept = !secret
    ? "Nothing was minted, and nothing was kept."
    : !sendable
      ? "What it returned is not a token, so nothing could be revoked with it, and nothing was kept — check Account → Personal access tokens in the web app for a token you did not expect."
      : outcome === "revoked" || outcome === "already_invalid"
        ? "What it minted was revoked, and nothing was kept."
        : "Nothing was kept on this machine, but the deployment did not confirm revoking what it minted — revoke it in the web app: Account → Personal access tokens.";
  // A mismatch quotes what the deployment said (slugs, an expiry, an error
  // body), so both parts pass through `printable` here, the one place every
  // refusal is raised.
  throw refusal(printable(problem.what), {
    detail: `${printable(problem.why)}. ${kept}`,
    hint
  });
}

/**
 * What a refused mint tells the agent, with the kind of token named. The last
 * sentence is for the OPERATOR: an old deployment answers a mint with an
 * ordinary login, and that login replaces theirs for this host.
 */
export const discardHint = (tokens: string) =>
  `stop and tell your operator: the deployment's web app and server must both be on a version with ${tokens}. If an ordinary login was issued, it may have replaced theirs for this host — \`oxyc whoami\`, then \`oxyc login\` again`;

const SANDBOX_DISCARD_HINT = discardHint("sandbox agent tokens");

/**
 * The one line stdout carries. A SHELL EVALUATES IT, so the shape is checked
 * again here, beside the sink: whatever `mismatch` becomes, a value that is not
 * letters and digits never reaches this line.
 */
function exportLine(token: string): string {
  if (!isWellFormedSandboxToken(token)) {
    throw refusal("refusing to print a secret that is not a whole sandbox agent token", {
      detail: "Nothing was printed, and nothing was kept.",
      hint: "this is a bug in oxyc's own check, not something to retry — report it"
    });
  }
  return `export OXY_TOKEN=${token}\n`;
}

/**
 * What a refused exchange means for a mint. The deployment gives one answer,
 * `400 invalid_code`, for a code that is spent or expired AND for an approved
 * mint it can no longer honour: who may mint is checked again at the exchange,
 * and so is an organization's cap on a token's lifetime. Running the command
 * again fixes only the first, so the second is named.
 */
const MINT_CODE_REFUSED = {
  message: "the approval code was rejected, and no token was minted",
  detail:
    "a code is single-use and lives five minutes. The deployment answers the same when it can no longer mint what was approved: the approver lost access to one of the apps, or an organization caps a token's lifetime below the hours asked for.",
  hint: `run \`${MINT_PURPOSE.retry}\` again — with fewer --hours if an organization caps token lifetime`
} as const;

/**
 * Mint a sandbox agent token and print it.
 *
 * Needs no credential and reads none: the approval happens under the
 * operator's browser session.
 *
 * EVERY STRING THIS COMMAND PRINTS THAT A DEPLOYMENT SUPPLIED goes through
 * `printable`: the token's name, its expiry (a date string is not safe — the
 * parser accepts a parenthesised comment holding anything), whatever a refusal
 * quotes, and the body of an exchange that failed (`exchangeCliCode`).
 */
export async function runTokensCreateSandboxAgent(
  ctx: Context,
  flags: SandboxMintFlags,
  opts: LoginOptions = {}
): Promise<void> {
  const hostname = opts.hostname ?? osHostname();
  const ask = parseSandboxMint(flags, hostname);
  const target = ctx.target();

  log.info(
    `Asking ${target} for a sandbox agent token: ${ask.apps.join(", ")}, ${ask.hours} h. ` +
      "Your operator approves it once in the browser."
  );
  const pkce = createPkce();
  const callback = await browserRound(target, { ...opts, hostname }, pkce, {
    query: sandboxMintQuery(ask),
    purpose: MINT_PURPOSE
  });
  // A session token in the callback is a deployment with no exchange at all.
  // It is the approver's browser session, not something minted: dropped.
  if (callback.kind === "token") {
    return discard(target, {
      what: unsupported(target),
      why: "its /cli-auth page handed back a session token, so it predates revocable CLI tokens"
    });
  }

  const minted = await exchangeCliCode(target, callback.code, pkce.verifier, MINT_CODE_REFUSED);
  if (!minted) {
    return discard(target, {
      what: unsupported(target),
      why: "it issued a code and has no exchange route (POST /api/auth/cli/exchange answered 404)"
    });
  }
  // BEFORE ANYTHING IS PRINTED. See `tokens-sandbox-verify.ts`.
  const problem = await mismatch(target, ask, minted);
  if (problem) return discard(target, problem, minted.token);

  // STDOUT IS THIS LINE AND NOTHING ELSE, so it can be `eval`ed.
  process.stdout.write(exportLine(minted.token));

  const name = printable(
    typeof minted.described?.name === "string" ? minted.described.name : ask.name
  );
  process.stderr.write(
    `${out.green(`Minted sandbox agent token "${name}" for ${ask.apps.join(", ")}.`)}\n`
  );
  if (minted.expiresAt) log.info(`it expires ${printable(describeExpiry(minted.expiresAt))}`);
  log.info("shown once and not stored on this machine — export it for the task");
  log.info("when the task is done: `oxyc tokens revoke --current`");
}

/**
 * Revoke the token this invocation is using: `DELETE /api/auth/token`.
 *
 * FOR A TOKEN IN THE ENVIRONMENT. A login cached by `oxyc login` is ended by
 * `oxyc logout`, which also drops it from this machine — revoking it here
 * would leave a dead token in the credentials file.
 */
export async function runTokensRevokeCurrent(ctx: Context): Promise<void> {
  const target = ctx.target();
  const credential = await ctx.credential();
  const variable = ctx.flags.tokenEnv ?? "OXY_TOKEN";
  if (credential.source === "file") {
    throw usageError(
      "--current revokes a token from the environment, and this one is your cached login",
      `\`oxyc logout\` revokes it and removes it from this machine — or set ${variable} to the token to end`
    );
  }
  if (!isRevocable(credential.token)) {
    throw usageError(
      "this credential cannot revoke itself",
      "only an API token can (oxy_pat_, oxy_sat_, oxy_ci_, oxy_sbx_) — a legacy API key is revoked in the web app, and a session by signing out"
    );
  }

  const outcome = await revokeCallingToken(target, credential.token);
  forgetSandboxTokens();
  if (outcome === "revoked") {
    process.stderr.write(`${out.green(`Revoked the token in ${variable}.`)}\n`);
    log.info(`unset ${variable} — the deployment no longer accepts it`);
    return;
  }
  if (outcome === "already_invalid") {
    // The goal state, reached some other way: it expired or was revoked.
    log.info(`the token in ${variable} was already expired or revoked — nothing left to do`);
    return;
  }
  if (outcome === "unsupported") {
    throw new CliError(`${target} cannot revoke this token`, {
      code: ExitCode.NOT_FOUND,
      detail: "DELETE /api/auth/token answered 404 — the deployment predates API tokens."
    });
  }
  throw new CliError(`could not revoke the token in ${variable}`, {
    code: ExitCode.UNAVAILABLE,
    hint: "the deployment did not confirm — run it again; the token also ends on its own at its expiry"
  });
}
