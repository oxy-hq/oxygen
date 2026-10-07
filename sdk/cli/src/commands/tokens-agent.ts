/**
 * `oxyc tokens create --agent` — mint an agent token: a personal access token
 * (`oxy_pat_…`) that reaches everything its approver does, for hours.
 *
 * IT IS THE COMMAND AN AGENT RUNS FOR EVERYTHING THAT IS NOT A SANDBOX. For
 * building a custom app in a sandbox there is `--sandbox-agent`, whose token
 * reaches the named apps' sandboxes and nothing else. For reading data with
 * `oxyc api`, or reproducing a bug against live data, an agent used to borrow
 * its operator's cached `oxyc login`: everything they can do, with staff
 * standing, for months, on disk for any later process. This is the agent's
 * own credential instead — named for it, hours long, never stored.
 *
 * THE SAME LOOPBACK as the sandbox mint (`tokens-sandbox.ts`), with other
 * parameters. It holds no credential while it runs; the operator approves once
 * in the browser, under their own session; and stdout is the one line
 * `export OXY_TOKEN=…`, printed once.
 *
 * WHAT IT NEVER DOES: write the credentials file, or revoke another token.
 *
 * STANDING IS ASKED FOR, NEVER ASSUMED. Without `--standing` a token that came
 * back carrying staff or partner standing is refused. With it, the approver
 * still decides on the page, where the box starts empty: a token with less
 * than was asked is accepted, and the command says so.
 */

import { hostname as osHostname } from "node:os";
import {
  type BrowserPurpose,
  browserRound,
  exchangeCliCode,
  type LoginOptions
} from "../auth/login.js";
import { createPkce } from "../auth/pkce.js";
import { describeExpiry } from "../auth/token-api.js";
import { isWellFormedPersonalToken } from "../auth/token-kind.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { refusal } from "../util/errors.js";
import { printable } from "../util/printable.js";
import { unsupportedAgent, verifyAgentToken } from "./tokens-agent-verify.js";
import { discard, discardHint, parseHours, parseName } from "./tokens-sandbox.js";
import type { Mismatch } from "./tokens-sandbox-verify.js";

export interface AgentMintFlags {
  /** `--standing`: ask for the approver's staff or partner standing too. */
  standing?: boolean;
  /** `--hours`, as typed. */
  hours?: string;
  name?: string;
}

export interface AgentMintRequest {
  standing: boolean;
  hours: number;
  name: string;
}

const MINT_PURPOSE: BrowserPurpose = {
  opening: "approve an agent token",
  retry: "oxyc tokens create --agent",
  done: "Agent token approved ✓",
  waitingFor: "approve the token"
};

const DISCARD_HINT = discardHint("agent tokens");

/**
 * Validate what was asked for. EVERY USAGE ERROR BEFORE ANY BROWSER OPENS: a
 * request the server would answer `400 invalid_agent_token` must not cost the
 * operator an approval click first.
 */
export function parseAgentMint(flags: AgentMintFlags, hostname: string): AgentMintRequest {
  return {
    standing: flags.standing === true,
    hours: parseHours(flags.hours),
    name: parseName(flags.name, `agent on ${hostname}`)
  };
}

/**
 * The parameters added to `/cli-auth`, already encoded. `standing` is there
 * only when it was asked for: its absence is the "no", and the page reads
 * nothing else as a "yes".
 */
export function agentMintQuery(ask: AgentMintRequest): string {
  return [
    "kind=agent",
    `hours=${ask.hours}`,
    ...(ask.standing ? ["standing=1"] : []),
    `name=${encodeURIComponent(ask.name)}`
  ].join("&");
}

/**
 * The one line stdout carries. A SHELL EVALUATES IT, so the shape is checked
 * again here, beside the sink: whatever the verification becomes, a value that
 * is not letters and digits never reaches this line.
 */
function exportLine(token: string): string {
  if (!isWellFormedPersonalToken(token)) {
    throw refusal("refusing to print a secret that is not a whole personal access token", {
      detail: "Nothing was printed, and nothing was kept.",
      hint: "this is a bug in oxyc's own check, not something to retry — report it"
    });
  }
  return `export OXY_TOKEN=${token}\n`;
}

/**
 * What a refused exchange means here. As for the sandbox mint, the deployment
 * gives one answer, `400 invalid_code`, for a code that is spent or expired
 * AND for an approved mint it can no longer honour: an organization's cap on a
 * token's lifetime is checked again at the exchange.
 */
const MINT_CODE_REFUSED = {
  message: "the approval code was rejected, and no token was minted",
  detail:
    "a code is single-use and lives five minutes. The deployment answers the same when it can no longer mint what was approved: an organization the approver belongs to caps a token's lifetime below the hours asked for.",
  hint: `run \`${MINT_PURPOSE.retry}\` again — with fewer --hours if an organization caps token lifetime`
} as const;

/** Refuse what the deployment handed back, in an agent token's words. */
const refuse = (target: string, problem: Mismatch, secret?: string) =>
  discard(target, problem, secret, DISCARD_HINT);

/** "staff", "partner" or "staff and partner". */
const named = (standing: readonly string[]) => standing.join(" and ");

/**
 * Mint an agent token and print it.
 *
 * Needs no credential and reads none: the approval happens under the
 * operator's browser session.
 *
 * EVERY STRING THIS COMMAND PRINTS THAT A DEPLOYMENT SUPPLIED goes through
 * `printable`: the token's name, its owner, its expiry, and whatever a refusal
 * quotes.
 */
export async function runTokensCreateAgent(
  ctx: Context,
  flags: AgentMintFlags,
  opts: LoginOptions = {}
): Promise<void> {
  const hostname = opts.hostname ?? osHostname();
  const ask = parseAgentMint(flags, hostname);
  const target = ctx.target();

  log.info(
    `Asking ${target} for an agent token: everything the approver can reach` +
      `${ask.standing ? ", with their staff or partner access if they tick it" : ""}, ${ask.hours} h. ` +
      "Your operator approves it once in the browser."
  );
  const pkce = createPkce();
  const callback = await browserRound(target, { ...opts, hostname }, pkce, {
    query: agentMintQuery(ask),
    purpose: MINT_PURPOSE
  });
  // A session token in the callback is a deployment with no exchange at all.
  // It is the approver's browser session, not something minted: dropped.
  if (callback.kind === "token") {
    return refuse(target, {
      what: unsupportedAgent(target),
      why: "its /cli-auth page handed back a session token, so it predates revocable CLI tokens"
    });
  }

  const minted = await exchangeCliCode(target, callback.code, pkce.verifier, MINT_CODE_REFUSED);
  if (!minted) {
    return refuse(target, {
      what: unsupportedAgent(target),
      why: "it issued a code and has no exchange route (POST /api/auth/cli/exchange answered 404)"
    });
  }
  // BEFORE ANYTHING IS PRINTED. See `tokens-agent-verify.ts`.
  const verdict = await verifyAgentToken(target, ask, minted);
  if ("problem" in verdict) return refuse(target, verdict.problem, minted.token);

  // STDOUT IS THIS LINE AND NOTHING ELSE, so it can be `eval`ed.
  process.stdout.write(exportLine(minted.token));

  const { token, standing } = verdict;
  const name = printable(typeof token.name === "string" ? token.name : ask.name);
  const owner = printable(
    typeof token.owner?.label === "string" ? token.owner.label : "its approver"
  );
  process.stderr.write(`${out.green(`Minted agent token "${name}", acting as ${owner}.`)}\n`);
  if (standing.length > 0) {
    log.info(`it reaches everything ${owner} does, ${named(standing)} access included`);
  } else if (ask.standing) {
    // Less than was asked for, which is the approver's to decide. NOT `info`:
    // an agent that planned on staff access has to hear this under --quiet too.
    log.warn(
      `--standing was asked for and the approver did not include it: the token reaches only what ${owner}'s organization memberships do`
    );
  } else {
    log.info(`it reaches what ${owner}'s organization memberships do`);
  }
  log.info(
    `it expires ${printable(describeExpiry(token.expires_at))}, and cannot be extended or renewed`
  );
  log.info("shown once and not stored on this machine — export it for the task");
  log.info(
    "pass --token-env OXY_TOKEN to each command, so one that lost the variable exits 4 instead of running on a cached login"
  );
  log.info("to end it: `oxyc tokens revoke --current`");
}
