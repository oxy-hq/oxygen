/**
 * `oxyc tokens list | revoke <id> | create` — personal access tokens.
 *
 * The plural manages; the singular `oxyc token` prints the one in use.
 *
 * TOKENS ONLY. A legacy API key (`oxy_<hex>`) is not a token: the server never
 * lists one under `/api/user/tokens` and answers 404 for its id there, so
 * nothing here handles one. They live in the web app, under
 * Settings → Workspace → Legacy API keys.
 *
 * MANAGEMENT IS BROWSER-SESSION-ONLY ON THE SERVER, and that is a security
 * property rather than a gap to route around: a token that could list, mint or
 * revoke tokens would make one leaked credential a way to outlive its own
 * revocation. So `/api/user/tokens` answers a token caller with
 * 403 `session_required` — and since `oxyc login` now caches a token, that is
 * what `list` and `revoke` will usually meet. They say so and point at the
 * web app. `create` never calls the API at all; it opens the page.
 *
 * They still WORK for a credentials file holding a session token, which is
 * every login made before the deployment gained the exchange.
 */

import { type ApiResponse, errorForResponse, parseJson, request } from "../api/request.js";
import { openBrowser } from "../auth/login.js";
import { describeExpiry, describeReach, normalizeToken, type Token } from "../auth/token-api.js";
import type { Context } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { table } from "../ui/render.js";
import { out } from "../ui/tty.js";
import { CliError, ExitCode } from "../util/errors.js";

/**
 * Account → Personal access tokens, as a link.
 *
 * `?settings=<section>` is the web app's deep link into its settings dialog
 * (`useSettingsDeepLink`); an unknown section is dropped rather than opening
 * the wrong one, so a deployment without the page lands on Home.
 */
export const TOKENS_SETTINGS_SECTION = "account.tokens";

export function tokensPageUrl(target: string): string {
  return `${target.replace(/\/+$/, "")}/?settings=${TOKENS_SETTINGS_SECTION}`;
}

/** Turn a refused management call into the error that says where to go. */
function managementError(response: ApiResponse, target: string, what: string): CliError {
  const body = parseJson(response.body) as { code?: string } | undefined;
  if (response.status === 403 && body?.code === "session_required") {
    return new CliError(`${what} needs a browser session, and this credential is a token`, {
      code: ExitCode.AUTH,
      detail:
        "a token cannot list, create or revoke tokens — otherwise a leaked one could outlive its own revocation.",
      hint:
        `manage them in the web app: ${tokensPageUrl(target)}   (Account → Personal access tokens)\n` +
        "to end the token this machine is using: `oxyc logout`"
    });
  }
  if (response.status === 404 && what === "listing tokens") {
    return new CliError("this deployment has no personal access tokens", {
      code: ExitCode.NOT_FOUND,
      detail: `GET /api/user/tokens answered 404 on ${target} — it predates them.`,
      hint: "its API keys are per workspace: Settings → Workspace → API Keys"
    });
  }
  return errorForResponse(response);
}

function ok(response: ApiResponse): boolean {
  return response.status >= 200 && response.status < 300;
}

/** One line of reach for a table cell; the full list is `oxyc whoami`'s job. */
function reachCell(token: Token): string {
  const [first = "", ...rest] = describeReach(token);
  return rest.length > 0 ? `${first} (+${rest.length} more)` : first;
}

export async function runTokensList(ctx: Context, json: boolean): Promise<void> {
  const target = ctx.target();
  const response = await request({
    target,
    path: "/api/user/tokens",
    method: "GET",
    bearer: await ctx.bearer(),
    timeoutMs: 30_000
  });
  if (!ok(response)) throw managementError(response, target, "listing tokens");

  if (json) {
    process.stdout.write(`${response.body.trim()}\n`);
    return;
  }
  const tokens = (
    (parseJson(response.body) as { tokens?: Partial<Token>[] } | undefined)?.tokens ?? []
  ).map(normalizeToken);
  if (tokens.length === 0) {
    log.info(`no tokens yet — \`oxyc tokens create\` opens the page that makes one`);
    return;
  }
  process.stdout.write(
    `${table(tokens, [
      { header: "id", value: (t) => t.id },
      { header: "name", value: (t) => t.name },
      { header: "kind", value: (t) => t.kind },
      { header: "reach", value: reachCell },
      { header: "expires", value: (t) => describeExpiry(t.expires_at) },
      { header: "last used", value: (t) => t.last_used_at?.slice(0, 10) ?? "never" },
      { header: "status", value: (t) => t.status }
    ])}\n`
  );
}

export async function runTokensRevoke(ctx: Context, id: string): Promise<void> {
  const target = ctx.target();
  const response = await request({
    target,
    path: `/api/user/tokens/${encodeURIComponent(id)}`,
    method: "DELETE",
    bearer: await ctx.bearer(),
    timeoutMs: 30_000
  });
  if (response.status === 404) {
    throw new CliError(`no token ${id} of yours on ${target}`, {
      code: ExitCode.NOT_FOUND,
      hint:
        "`oxyc tokens list` shows the ids — someone else's token answers the same way\n" +
        "a legacy API key is not a token: revoke one in the web app, Settings → Workspace → Legacy API keys"
    });
  }
  if (!ok(response)) throw managementError(response, target, "revoking a token");
  process.stderr.write(`${out.green(`Revoked ${id}.`)}\n`);
}

/**
 * Open the page that creates one.
 *
 * Not a `POST /api/user/tokens` from here, even where the credential is a
 * session: the secret is shown once, and a terminal's scrollback, a CI log and
 * a shell history are three places it should not be shown at all.
 */
export function runTokensCreate(ctx: Context, open: (url: string) => void = openBrowser): void {
  const url = tokensPageUrl(ctx.target());
  log.info(`Opening ${url} — Account → Personal access tokens.`);
  log.info("If it doesn't open automatically, paste that URL into your browser.");
  open(url);
  process.stdout.write(`${url}\n`);
}
