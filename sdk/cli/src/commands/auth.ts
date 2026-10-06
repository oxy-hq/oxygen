/**
 * `oxyc login`, `logout`, `whoami`, `token`.
 *
 * These write the SAME file the Rust `oxy` binary uses, so logging in with
 * either tool authenticates both. That is the whole reason `oxyc` can be
 * installed on its own: a developer with only `npx @oxy-hq/cli` is not a
 * second-class citizen, and one with both never logs in twice.
 */

import { parseJson, request } from "../api/request.js";
import { describeSandboxToken } from "../apps/sandbox-token.js";
import { clearCredential, loadCredential } from "../auth/credentials.js";
import { keepPastExit } from "../auth/exit-revoke.js";
import { adminStatusLine, login } from "../auth/login.js";
import {
  describeExpiry,
  describeReach,
  introspectToken,
  revokeCallingToken,
  type Token
} from "../auth/token-api.js";
import { isRevocable, isSandboxAgentToken } from "../auth/token-kind.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode } from "../util/errors.js";
import { runAssumeStart } from "./assume.js";

/**
 * Log into one deployment, or several.
 *
 * MULTI-ENV IS SEQUENTIAL, not concurrent: each one opens a browser and waits
 * for its callback, and two tabs racing for the same loopback port is a login
 * that fails for a reason nobody can read. The Rust `oxy login --env dev,staging`
 * did the same, and its logins are still in the same credential file.
 *
 * A FAILURE DOES NOT ABANDON THE REST. Logging into dev and staging is a
 * sequence of independent acts, and stopping at the first would leave you
 * having typed a browser flow for nothing. Each is reported as it lands, and
 * the exit code is non-zero if any failed.
 */
export async function runLogin(
  ctx: Context,
  envs: string[],
  assume?: { org?: string; reason: string }
): Promise<void> {
  const targets = envs.length > 0 ? envs.map((e) => ctx.withEnv(e)) : [ctx];
  const failures: string[] = [];

  for (const one of targets) {
    const target = one.target();
    try {
      const { user } = await login(target);
      process.stderr.write(`${out.green(`Logged in as ${user.email} (${target}).`)}\n`);
      process.stderr.write(`${adminStatusLine(user)}\n`);
      if (user.expires_at) {
        log.info(
          `the token expires ${describeExpiry(user.expires_at)} — \`oxyc logout\` revokes it sooner`
        );
      }
    } catch (cause) {
      failures.push(target);
      log.warn(`could not log into ${target}: ${(cause as Error).message}`);
      continue;
    }

    // Single-target by construction — `main.ts` refuses the multi-env case
    // before any browser opens, which is where a usage error belongs.
    if (assume) {
      await runAssumeStart(one, assume.org, assume.reason);
    }
  }

  if (failures.length > 0) {
    throw new CliError(`could not log into ${failures.length} of ${targets.length} deployment(s)`, {
      code: ExitCode.FAILURE,
      detail: failures.join("\n")
    });
  }
}

/**
 * Revoke the cached token on the server, then forget it here.
 *
 * IN THAT ORDER, and the second step never waits on the first succeeding. A
 * token `oxyc login` minted is a row the deployment can end, so logging out
 * ends it rather than leaving a ninety-day credential behind in a backup of
 * this file. But the revoke is best-effort: a session token cannot be revoked
 * at all, an older deployment has no route for it, and an unreachable one must
 * not leave you unable to log out. Whatever the server says, the entry goes.
 *
 * Only the CACHED token is touched. `OXY_TOKEN` is the caller's own, and
 * "log out" does not mean "kill the secret my CI is using".
 */
export async function runLogout(ctx: Context): Promise<void> {
  const target = ctx.target();
  const token = loadCredential(target)?.token?.trim();
  const outcome = token ? await revokeCallingToken(target, token) : undefined;

  if (!clearCredential(target)) {
    log.info(`no cached credential for ${target}`);
    return;
  }
  process.stderr.write(`${out.green(`Logged out of ${target}.`)}\n`);
  if (outcome === "revoked") {
    log.info("the token was revoked on the server");
  } else if (outcome === "legacy") {
    // 409 `legacy_immutable`: the credential is a legacy API key (`oxy_<hex>`),
    // which cannot end itself. A legacy API key is not a token, so it is not
    // called one here, and it is not revoked where tokens are.
    log.warn(
      "the credential was removed from this machine, but it is a legacy API key and cannot revoke itself"
    );
    log.hint("revoke it in the web app: Settings → Workspace → Legacy API keys");
  } else if (token && isRevocable(token) && outcome !== "already_invalid") {
    // Said only for a token that SHOULD have been revocable. A session token
    // was never going to be, and saying so on every logout from an older
    // deployment would be noise about something nobody can change.
    log.warn("the token was removed from this machine, but the server did not confirm a revoke");
    log.hint("revoke it in the web app: Account → Personal access tokens");
  }
}

/**
 * Who this token is, and what it can reach.
 *
 * Deliberately makes a live call rather than printing the cached email: the
 * cached value is what was true at login, and the failure this command exists
 * to diagnose — an expired token, a revoked grant, a missing assume session —
 * is invisible in the cache. A `whoami` that reads a file cannot tell you the
 * token stopped working.
 */
export async function runWhoami(ctx: Context, json: boolean): Promise<void> {
  const target = ctx.target();
  const bearer = await ctx.bearer();

  // `/api/user` answers a sandbox agent token 404, like every path outside the
  // sandbox loop. What that token is — kind, expiry, apps, minter — is the
  // introspection route's document, and `--json` prints it as the server sent it.
  if (isSandboxAgentToken(bearer)) {
    const described = await describeSandboxToken(target, bearer, { fresh: true });
    const text = json
      ? described.body.trim()
      : sandboxTokenLines(target, described.token).join("\n");
    process.stdout.write(`${text}\n`);
    return;
  }

  const response = await request({
    target,
    path: "/api/user",
    method: "GET",
    bearer,
    timeoutMs: 30_000
  });
  if (response.status === 401 || response.status === 403) {
    throw new CliError(`the cached token for ${target} is no longer accepted`, {
      code: ExitCode.AUTH,
      hint: `oxyc login --env ${ctx.flags.env ?? "production"}`
    });
  }
  if (response.status < 200 || response.status >= 300) {
    throw new CliError(`could not read /api/user (${response.status})`, {
      code: ExitCode.UNAVAILABLE
    });
  }

  const payload = parseJson(response.body);

  // A 200 whose body is `null` is the shape an EXPIRED token produces here:
  // the request is accepted, no user resolves, and the server says so with a
  // null rather than a 401. Reporting that as success — falling back to the
  // cached email, which is still sitting in the credentials file — is the
  // precise failure this command exists to catch, and it is what the first
  // run of this code did. `login.rs` refuses the same shape at login time.
  if (payload === null || payload === undefined) {
    throw new CliError(`the token for ${target} no longer resolves to a user`, {
      code: ExitCode.AUTH,
      detail:
        "GET /api/user answered 200 with a null body, which is what an expired session looks like.",
      hint: `oxyc login --env ${ctx.flags.env ?? "production"}`
    });
  }

  if (json) {
    process.stdout.write(`${response.body.trim()}\n`);
    return;
  }

  const user = payload as Record<string, unknown>;
  const cached = loadCredential(target);
  const lines = [
    `${out.bold("target")}      ${target}`,
    `${out.bold("email")}       ${typeof user.email === "string" ? user.email : (cached?.email ?? "unknown")}`,
    `${out.bold("app admin")}   ${user.is_app_admin ? "yes" : "no"}`
  ];
  if (typeof user.id === "string") lines.push(`${out.bold("user id")}     ${user.id}`);
  const customer = ctx.customer();
  if (customer)
    lines.push(`${out.bold("customer")}    ${customer.name}  (from the repo you are in)`);
  lines.push(...(await tokenLines(target, bearer)));
  process.stdout.write(`${lines.join("\n")}\n`);
}

/**
 * What the calling CREDENTIAL is and can reach — the half of "who am I" that
 * `/api/user` cannot answer, because a narrowed token is still its owner.
 *
 * Empty against a deployment with no `GET /api/auth/token`: nothing to add is
 * the right output there, not an error under a `whoami` that just succeeded.
 */
async function tokenLines(target: string, bearer: string): Promise<string[]> {
  const found = await introspectToken(target, bearer);
  if (found.kind === "session") {
    return [`${out.bold("credential")}  a browser session — everything you can reach`];
  }
  if (found.kind !== "token") return [];
  return credentialLines(found.token);
}

/**
 * The lines `whoami` prints for the row behind the calling credential.
 *
 * A LEGACY API KEY IS NOT A TOKEN and is never labelled one: the route answers
 * for it (`kind: "legacy_key"`) so this can say what it is. Its reach is stated
 * rather than read off the row — the row carries `all_access` and both standing
 * flags for every legacy API key, which says how it is stored, not what its
 * owner holds.
 */
export function credentialLines(token: Token): string[] {
  const masked = `${token.display_prefix}…${token.last_four}`;
  if (token.kind === "legacy_key") {
    return [
      `${out.bold("credential")}  Legacy API key: ${token.name}  (${masked})`,
      `${out.bold("reach")}       everything its owner can — it can't be limited to workspaces`,
      `${out.bold("expires")}     ${describeExpiry(token.expires_at)}`
    ];
  }
  const [first = "", ...rest] = describeReach(token);
  return [
    `${out.bold("token")}       ${token.name}  (${token.kind}, ${masked})`,
    `${out.bold("reach")}       ${first}`,
    ...rest.map((line) => `            ${line}`),
    `${out.bold("expires")}     ${describeExpiry(token.expires_at)}`
  ];
}

/**
 * `whoami` for a sandbox agent token: what an agent checks before it starts —
 * that the app it was asked to work on is listed, and that the token outlives
 * the task.
 */
export function sandboxTokenLines(target: string, token: Token): string[] {
  const masked = `${token.display_prefix}…${token.last_four}`;
  const [first = "none", ...rest] = (token.apps ?? []).map((a) => `${a.org_slug}/${a.slug}`);
  return [
    `${out.bold("target")}      ${target}`,
    `${out.bold("token")}       ${token.name}  (${token.kind}, ${masked})`,
    `${out.bold("minted by")}   ${token.minter?.email ?? token.owner?.label ?? "unknown"}`,
    `${out.bold("apps")}        ${first}`,
    ...rest.map((app) => `            ${app}`),
    `${out.bold("reach")}       the dev-<handle> sandboxes it creates in those apps — nothing else`,
    `${out.bold("expires")}     ${describeExpiry(token.expires_at)}`
  ];
}

/**
 * Print the bearer, for a raw `curl`.
 *
 * It exists because the alternative is people copying tokens out of the
 * credentials file by hand, which is worse in every way — including that they
 * then paste the wrong host's.
 *
 * Inside a GitHub Actions job with nothing stored, this prints the token the
 * job's OIDC identity exchanges for — `gh auth token`'s shape. THAT ONE IS NOT
 * REVOKED ON EXIT, alone among the commands that mint: the output is the
 * token, and a token dead before the caller reads it is no output at all. It
 * expires in fifteen minutes regardless, and the expiry goes to stderr so
 * stdout stays exactly the token.
 */
export async function runToken(ctx: Context): Promise<void> {
  const credential = await ctx.credential();
  if (credential.source === "oidc") {
    keepPastExit(credential.token);
    const who = credential.serviceAccount ? ` for ${credential.serviceAccount}` : "";
    log.info(`exchanged this job's GitHub OIDC token${who} — not revoked on exit`);
  }
  if (credential.expiresAt) log.info(`expires ${describeExpiry(credential.expiresAt)}`);
  process.stdout.write(`${credential.token}\n`);
}
