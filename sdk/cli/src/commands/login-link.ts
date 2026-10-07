/**
 * `oxyc login-link` — a one-time URL that signs a browser in with the
 * credential `oxyc` is running on.
 *
 * FOR A BROWSER NOBODY IS SITTING AT. Sign-in is passwordless — Google or
 * GitHub OAuth, or a magic link — and an automation agent driving Playwright
 * can finish neither: it cannot get through a provider's consent screen and it
 * has no inbox to read. On a deployed environment that left it with no way to
 * a signed-in page at all. With this the whole sign-in is one navigation:
 *
 *     browser_navigate("$(oxyc login-link --env dev --next /ide)")
 *
 * THE BEARER NEVER LEAVES THIS PROCESS. It is spent on one request, for a
 * TICKET: single-use and minutes long. That is why the URL can be printed,
 * pasted and logged, and why the token is never part of it.
 *
 * EVERYTHING AFTER THE `#` STAYS IN THE BROWSER. The ticket and `--next` ride
 * the fragment, which no request carries, so neither reaches an access log, a
 * proxy or a `Referer`.
 *
 * ONLY A PERSONAL ACCESS TOKEN CAN ASK (`oxy_pat_…`, what `oxyc login` mints).
 * A browser session is a person's, and a service account, a CI job, a sandbox
 * agent and a legacy API key are not one. The session is bounded by the token:
 * it can do what the token can do, it cannot manage tokens, and it ends when
 * the token is revoked.
 */

import {
  type ApiResponse,
  buildUrl,
  errorForResponse,
  parseJson,
  request
} from "../api/request.js";
import { refuseSandboxToken } from "../apps/sandbox-token.js";
import { openBrowser } from "../auth/login.js";
import { describeExpiry } from "../auth/token-api.js";
import { credentialShape } from "../auth/token-kind.js";
import { type Context, envOnly, type ResolvedCredential } from "../context/resolve.js";
import * as log from "../ui/log.js";
import { CliError, ExitCode, usageError } from "../util/errors.js";
import { printable } from "../util/printable.js";

/** Trades the calling token for a ticket. No body: the bearer is the whole request. */
const TICKET_PATH = "/api/auth/browser-ticket";

export interface LoginLinkOptions {
  /** `--next`: where the browser lands once it is signed in. */
  next?: string;
  json?: boolean;
  open?: boolean;
}

/** What `--json` prints. */
export interface LoginLink {
  url: string;
  /** RFC 3339, as the server sent it: when the LINK stops working. */
  expires_at: string;
  /** How long the browser session it opens lasts. */
  session_seconds: number;
}

/**
 * Whether `value` is a path on the deployment and can be read as nothing else.
 *
 * A single leading `/`, because `//host/…` is another origin with the scheme
 * left off. No backslash, because a browser reads one as a slash and `/\host`
 * is then the same thing. No control character, because a browser DROPS a tab
 * or a newline before it parses, which makes `/<tab>/host` a third spelling.
 */
function isLocalPath(value: string): boolean {
  return (
    value.startsWith("/") &&
    !value.startsWith("//") &&
    !value.includes("\\") &&
    printable(value) === value
  );
}

/**
 * `--next`, checked before anything is asked of the deployment.
 *
 * A sign-in link is a redirect waiting to happen, so one built here never
 * carries a destination off the deployment — whatever the page would do with
 * it. A usage error rather than a request: it spends no ticket, and it is
 * reported where there is a terminal to read it rather than minutes later in a
 * browser that landed somewhere else.
 */
export function parseNext(next: string | undefined): string | undefined {
  if (next === undefined) return undefined;
  if (!isLocalPath(next)) {
    throw usageError(
      `--next ${JSON.stringify(next)} is not a path on the deployment`,
      "it starts with a single / — /ide, /a/store/orders?tab=open — and is never a URL, //host or a backslash"
    );
  }
  return next;
}

/**
 * The link: the target, the path the server chose, and `--next` beside the
 * ticket.
 *
 * `next` GOES IN THE FRAGMENT, wherever the server put the ticket — after a
 * `#` it is never sent, and in a query string it would be in every access log
 * between here and the deployment. Escaped whole, so a `?`, `&` or `#` of its
 * own stays part of the destination instead of ending it.
 */
export function loginLinkUrl(target: string, path: string, next?: string): string {
  const url = buildUrl(target, path);
  if (next === undefined) return url;
  return `${url}${url.includes("#") ? "&" : "#"}next=${encodeURIComponent(next)}`;
}

interface Ticket {
  path: string;
  expiresAt: string;
  sessionSeconds: number;
}

/**
 * The three fields of the answer this command uses, or `undefined` when any is
 * missing or not what it should be. `ticket` is not read: it is inside `path`.
 *
 * `path` IS CHECKED, NOT TRUSTED. It is appended to the target and the result
 * is handed to a browser, so a deployment that answered `@elsewhere/…` would
 * have this print a link to another host under its own name. No whitespace
 * either: stdout is one word, for `$(…)`.
 */
function readTicket(body: string): Ticket | undefined {
  const sent = (parseJson(body) ?? {}) as Record<string, unknown>;
  const { path, expires_at: expiresAt, session_seconds: sessionSeconds } = sent;
  if (typeof path !== "string" || !isLocalPath(path) || /\s/.test(path)) return undefined;
  if (typeof expiresAt !== "string" || Number.isNaN(Date.parse(expiresAt))) return undefined;
  if (typeof sessionSeconds !== "number" || !Number.isFinite(sessionSeconds)) return undefined;
  if (sessionSeconds <= 0) return undefined;
  return { path, expiresAt, sessionSeconds };
}

/** `8 hours`, `1 hour`, `1.5 hours` — and minutes for a session shorter than one. */
function describeHours(seconds: number): string {
  if (seconds < 3600) return `${Math.max(1, Math.round(seconds / 60))} min`;
  const hours = Math.round(seconds / 360) / 10;
  return `${hours} ${hours === 1 ? "hour" : "hours"}`;
}

/** What the credential is, read off its prefix, for the sentence that says it cannot ask. */
function describeCredential(token: string): string {
  switch (credentialShape(token)) {
    case "service_account":
      return "a service account token";
    case "ci":
      return "a CI token";
    case "sandbox_agent":
      return "a sandbox agent token";
    case "publish":
      return "a publish token";
    case "legacy_key":
      return "a legacy API key";
    case "session":
      return "a session token";
    case "personal":
      return "not accepted as one";
  }
}

/**
 * What to do about a credential that cannot ask.
 *
 * WHICH SENTENCE DEPENDS ON WHERE THE CREDENTIAL CAME FROM. The variable wins
 * over the login cache, so "run `oxyc login`" alone would send someone whose
 * `OXY_TOKEN` holds a service account token through a browser flow and
 * straight back to this error. And a NAMED variable is the only source, so
 * unsetting it leads to no login at all.
 */
function nextStep(ctx: Context, source: ResolvedCredential["source"]): string {
  const variable = ctx.flags.tokenEnv ?? "OXY_TOKEN";
  const login = `oxyc login --env ${ctx.flags.env ?? "production"}`;
  if (source !== "env") return `run \`${login}\`   (or put a personal access token in ${variable})`;
  const put = `put a personal access token in ${variable}`;
  return envOnly(ctx.flags) ? put : `${put} — or unset it and run \`${login}\``;
}

/**
 * Turn a refused ticket request into the error that says what to do.
 *
 * `no_token` (404: the caller is no API token at all) and
 * `personal_token_required` (403: it is one, of the wrong kind) ARE BOTH EXIT
 * 4, though the first arrives as a 404. The code an agent branches on answers
 * "what next", and next is a different credential — 5 would tell it to stop.
 */
function refused(response: ApiResponse, ctx: Context, credential: ResolvedCredential): CliError {
  const target = ctx.target();
  const code = (parseJson(response.body) as { code?: unknown } | null | undefined)?.code;

  if (code === "no_token" || code === "personal_token_required") {
    const what = describeCredential(credential.token);
    return new CliError(
      `a sign-in link needs a personal access token, and this credential is ${what}`,
      {
        code: ExitCode.AUTH,
        detail:
          "a browser session is a person's, so only a person's own token (oxy_pat_…, what `oxyc login` mints) can open one.",
        remedy: nextStep(ctx, credential.source),
        serverCode: code
      }
    );
  }
  if (response.status === 401) {
    return new CliError(`the credential for ${target} is no longer accepted`, {
      code: ExitCode.AUTH,
      remedy: nextStep(ctx, credential.source)
    });
  }
  // A deployment that predates the route answers a bare 404 (a 405 where
  // something in front of it answers instead). Reported as "not found" it
  // would read as a wrong path — and the path is this command's, not the
  // caller's.
  if ((response.status === 404 || response.status === 405) && typeof code !== "string") {
    return new CliError(`${target} does not support sign-in links yet`, {
      code: ExitCode.NOT_FOUND,
      detail: `POST ${TICKET_PATH} answered ${response.status} — the deployment predates them.`,
      hint: "nothing to fix on this machine: sign in there by hand until it is upgraded"
    });
  }
  return errorForResponse(response);
}

/**
 * Ask for a ticket and print the link it makes.
 *
 * STDOUT IS THE URL AND A NEWLINE — or, with `--json`, one object — so it can
 * be `$(…)`ed straight into a navigation. What a person wants to know about it
 * goes to stderr.
 */
export async function runLoginLink(
  ctx: Context,
  opts: LoginLinkOptions = {},
  open: (url: string) => void = openBrowser
): Promise<void> {
  // BEFORE THE CREDENTIAL IS RESOLVED, as every usage error is: a mistyped
  // flag answers "you called it wrong" (2), not "log in" (4).
  const next = parseNext(opts.next);
  // Said here rather than read off the response. A deployment answers this
  // token 404 on paths outside the sandbox loop, which is also what one with
  // no such route says — and exit 4 under this token means "stop", never the
  // "log in" every other credential is told below.
  refuseSandboxToken(
    ctx.storedBearer(),
    "open a browser session",
    "it does the sandbox loop and nothing else — a sign-in link needs a personal access token, which is your operator's to supply"
  );

  const target = ctx.target();
  const credential = await ctx.credential();
  const response = await request({
    target,
    path: TICKET_PATH,
    method: "POST",
    bearer: credential.token,
    timeoutMs: 30_000
  });
  if (response.status < 200 || response.status >= 300) throw refused(response, ctx, credential);

  const ticket = readTicket(response.body);
  if (!ticket) {
    throw new CliError(`${target} answered without a usable sign-in link`, {
      code: ExitCode.FAILURE,
      detail: `POST ${TICKET_PATH} answered ${response.status}, but not with a \`path\` on the deployment, an \`expires_at\` and a \`session_seconds\`.`
    });
  }

  const link: LoginLink = {
    url: loginLinkUrl(target, ticket.path, next),
    expires_at: ticket.expiresAt,
    session_seconds: ticket.sessionSeconds
  };
  const expires = printable(describeExpiry(link.expires_at));
  log.info(`a sign-in link for ${target} — it works once and expires ${expires}`);
  log.info(
    `it opens a browser session of ${describeHours(link.session_seconds)} that can do what this token can do and nothing more, and ends when the token is revoked`
  );
  if (opts.open) {
    log.info("opening it in your browser — which uses it, so the URL below will not work again");
    open(link.url);
  }
  process.stdout.write(`${opts.json ? JSON.stringify(link) : link.url}\n`);
}
