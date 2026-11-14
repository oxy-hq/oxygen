/**
 * Approval by device code: `--no-browser` on `oxyc login` and on
 * `oxyc tokens create --agent|--sandbox-agent`.
 *
 * FOR A MACHINE NO BROWSER CAN REACH — a cloud container, a CI runner, a
 * remote dev box. The loopback flow (`login.ts`) hands the code back through
 * the approver's browser to `127.0.0.1:<port>`, and there that port is on
 * another machine. Here nothing comes back through the browser:
 *
 *   1. `POST /api/auth/cli/device` with the PKCE challenge, the hostname and
 *      the ASK — the same `/cli-auth` parameters the loopback URL carries, so
 *      the page reads a device request exactly as it reads a link. The answer
 *      is a secret device code and a short user code.
 *   2. The URL and the user code go to stderr. A person opens the URL on any
 *      device, signed in, types the code oxyc printed, and approves. The page
 *      never shows the code: reading it off this terminal is the check.
 *      With `--approver <email>` the start also names who must approve, and
 *      the platform tells them itself (push, email, Slack, the inbox): the
 *      notice carries no code and links to the page without one, so they still
 *      type it from this terminal. Whether the email names anyone is never
 *      answered, so a typo is not told apart from a stranger.
 *   3. `POST /api/auth/cli/device/poll` with the device code and the PKCE
 *      verifier, every `interval` seconds, until the deployment answers with
 *      the token (the exchange's own body), a refusal, or a lapse.
 *
 * What it returns is what `exchangeCliCode` returns, so every caller checks
 * and prints the token exactly as it does after a loopback approval.
 * `undefined` is a deployment with no device-code route (404 on the start):
 * the caller says so in its own words.
 */

import { hostname as osHostname } from "node:os";
import { withUserAgent } from "../api/user-agent.js";
import * as log from "../ui/log.js";
import { CliError, ExitCode, exitCodeForStatus } from "../util/errors.js";
import { printable, printableLines } from "../util/printable.js";
import {
  type BrowserPurpose,
  base,
  type CodeRefused,
  LOGIN_CODE_REFUSED,
  LOGIN_PURPOSE,
  type LoginOptions,
  type MintBody,
  type Minted,
  mintedFrom,
  parseBody
} from "./login.js";
import { retryAfterSeconds } from "./oidc.js";
import type { Pkce } from "./pkce.js";

/** The page a person approves on. The user code rides in the URL, and is shown there to compare. */
export const DEVICE_PAGE = "/cli-auth/device";

/** What the start answers: one request, collectable by whoever holds the verifier. */
export interface DeviceRequest {
  device_code: string;
  user_code: string;
  expires_in: number;
  interval: number;
}

/** RFC 8628 §3.5: a `slow_down` adds five seconds to every later wait. */
const SLOW_DOWN_STEP_S = 5;
/** Network failures in a row a poll survives before it gives up. */
const MAX_POLL_FAILURES = 3;
/**
 * Seconds past the request's own lifetime the poll keeps asking. The
 * deployment answers `expired_token` at the lifetime; this is the bound for a
 * deployment that keeps answering `pending` or 429 instead.
 */
const DEADLINE_GRACE_S = 30;

/**
 * A real wait between polls. Not `unref`ed: while a login waits for approval
 * the timer is the only thing keeping the process alive, and an unref'd one
 * let Node exit mid-wait with the request still pending.
 */
export const defaultSleep = (ms: number) =>
  new Promise<void>((resolve) => {
    setTimeout(resolve, ms);
  });

/** `k=v&k=v`, as a `/cli-auth` URL carries it, as the object the start sends. */
export function askFromQuery(query: string | undefined): Record<string, string> {
  return query ? Object.fromEntries(new URLSearchParams(query)) : {};
}

/** Where the person approves this request. */
export function deviceUrl(target: string, userCode: string): string {
  return `${base(target)}${DEVICE_PAGE}?code=${encodeURIComponent(userCode)}`;
}

async function post(url: string, body: unknown): Promise<Response> {
  return fetch(url, {
    method: "POST",
    headers: withUserAgent({ "content-type": "application/json", accept: "application/json" }),
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(30_000)
  });
}

/** Start a request. `undefined` when the deployment has no device-code route. */
async function startRequest(
  target: string,
  pkce: Pkce,
  hostname: string,
  query: string | undefined,
  approver: string | undefined
): Promise<DeviceRequest | undefined> {
  const url = `${base(target)}/api/auth/cli/device`;
  const ask = askFromQuery(query);
  let response: Response;
  try {
    response = await post(url, {
      code_challenge: pkce.challenge,
      hostname,
      ask,
      ...(approver ? { approver } : {})
    });
  } catch (cause) {
    throw new CliError(`POST ${url} failed: ${(cause as Error).message}`, {
      code: ExitCode.UNAVAILABLE
    });
  }
  if (response.status === 404) return undefined;
  const text = await response.text();
  if (!response.ok) {
    throw new CliError(`the deployment refused the request (${response.status})`, {
      code: exitCodeForStatus(response.status),
      detail: printableLines(text.slice(0, 2000)).trim() || undefined
    });
  }
  const body = parseBody(text) as Partial<DeviceRequest>;
  if (typeof body.device_code !== "string" || typeof body.user_code !== "string") {
    throw new CliError("the deployment started no device-code request", {
      code: ExitCode.UNAVAILABLE
    });
  }
  return {
    device_code: body.device_code,
    user_code: body.user_code,
    expires_in: Number(body.expires_in) > 0 ? Number(body.expires_in) : 600,
    interval: Number(body.interval) > 0 ? Number(body.interval) : 5
  };
}

/** Say where to approve. Stderr only: stdout stays the one line a shell evaluates. */
export function announce(
  target: string,
  started: DeviceRequest,
  purpose: BrowserPurpose,
  approver: string | undefined
): void {
  const code = printable(started.user_code);
  const url = deviceUrl(target, started.user_code);
  // `say`, not `info`: `--quiet` must not hide what the person has to act on.
  // The link carries the code, so opening it is the whole step; the bare page
  // and the code are the fallback for a device the link cannot reach.
  log.say(`To ${purpose.opening}, open this link on any device where you are signed in to Oxygen:`);
  log.say(`  ${url}`);
  log.say(`(or open ${base(target)}${DEVICE_PAGE} and type the code ${code})`);
  // The same words whether or not the email names an account: the deployment
  // does not say, and neither do we. The notice never carries the code
  // (RFC 8628 §5.4), so the one-step path for the approver is this link,
  // passed on by whoever ran the command.
  if (approver) {
    log.say(
      `Approval request sent to ${printable(approver)}. The notice has no code: send them the link above, or the code ${code}.`
    );
  }
}

/** The line a caller that waits prints after `announce`. */
export function announceWaiting(secondsLeft: number): void {
  log.info(`Waiting for approval (the request lapses in ${Math.round(secondsLeft / 60)} min)…`);
}

/**
 * One poll's answer: the token, or how long to wait before the next poll and
 * the interval to keep after it. A `slow_down` raises the interval for good;
 * a 429 from the per-client limiter waits once and keeps it.
 */
type Step = { minted: Minted } | { wait: number; interval: number };

/** Read one poll's answer. Throws on a refusal, a lapse, or a spent request. */
function stepFrom(
  response: Response,
  body: MintBody,
  text: string,
  interval: number,
  refused: CodeRefused,
  purpose: BrowserPurpose
): Step {
  if (response.ok) {
    if (body.status === "pending") return { wait: interval, interval };
    if (body.status === "slow_down") {
      const slower = interval + SLOW_DOWN_STEP_S;
      return { wait: slower, interval: slower };
    }
    return { minted: mintedFrom(body, "the approval returned no token") };
  }
  // Over the per-client budget, which agents sharing one address can be. The
  // request is still there, and may already be approved: wait it out.
  if (response.status === 429) {
    const wait = Math.max(interval, retryAfterSeconds(response.headers.get("retry-after")));
    return { wait, interval };
  }
  if (body.code === "access_denied") {
    throw new CliError("the request was refused, and no token was minted", {
      code: ExitCode.AUTH,
      detail: body.error ? `${printable(body.error)}.` : undefined,
      hint: `put the request right, then run \`${purpose.retry} --no-browser\` again`,
      serverCode: "access_denied"
    });
  }
  if (body.code === "expired_token") {
    throw new CliError(`timed out waiting for someone to ${purpose.waitingFor}`, {
      code: ExitCode.UNAVAILABLE,
      detail: "a request lives ten minutes.",
      hint: `run \`${purpose.retry} --no-browser\` again, and approve it sooner`,
      serverCode: "expired_token"
    });
  }
  if (body.code === "invalid_code") {
    throw new CliError(refused.message, {
      code: ExitCode.AUTH,
      detail: refused.detail,
      hint: refused.hint,
      serverCode: "invalid_code"
    });
  }
  throw new CliError(`polling for the approval failed (${response.status})`, {
    code: exitCodeForStatus(response.status),
    detail: printableLines(text.slice(0, 2000)).trim() || undefined
  });
}

/**
 * Poll until the request is decided, collecting the token once.
 *
 * `firstWait` is seconds before the first poll: the interval for a request
 * just started, `0` for one `oxyc login --no-wait` started earlier, which may
 * already be approved.
 */
export async function pollUntilDecided(
  target: string,
  verifier: string,
  started: DeviceRequest,
  wording: { refused: CodeRefused; purpose: BrowserPurpose },
  sleep: (ms: number) => Promise<void>,
  firstWait: number = started.interval
): Promise<Minted> {
  const url = `${base(target)}/api/auth/cli/device/poll`;
  const body = { device_code: started.device_code, code_verifier: verifier };
  let interval = started.interval;
  let wait = firstWait;
  let failures = 0;
  // Counted in the waits asked for, not the wall clock, so a test's `sleep`
  // drives it. Each request adds a little real time on top: a bound, not a timer.
  let waited = 0;
  for (;;) {
    if (waited + wait > started.expires_in + DEADLINE_GRACE_S) {
      throw new CliError(`timed out waiting for someone to ${wording.purpose.waitingFor}`, {
        code: ExitCode.UNAVAILABLE,
        detail: `the request lapsed after ${Math.round(started.expires_in / 60)} min with no decision.`,
        hint: `run \`${wording.purpose.retry} --no-browser\` again, and approve it sooner`,
        serverCode: "expired_token"
      });
    }
    if (wait > 0) await sleep(wait * 1000);
    waited += wait;
    let response: Response;
    try {
      response = await post(url, body);
    } catch (cause) {
      // A blip while a person reads the page is not a reason to lose the request.
      failures += 1;
      if (failures < MAX_POLL_FAILURES) continue;
      throw new CliError(`POST ${url} failed: ${(cause as Error).message}`, {
        code: ExitCode.UNAVAILABLE
      });
    }
    failures = 0;
    const text = await response.text();
    const step = stepFrom(
      response,
      parseBody(text),
      text,
      interval,
      wording.refused,
      wording.purpose
    );
    if ("minted" in step) return step.minted;
    ({ wait, interval } = step);
  }
}

/**
 * Start a device-code request, say where to approve it, and wait for it.
 *
 * `ask.query` is what a caller other than `oxyc login` adds — the `/cli-auth`
 * parameters, already encoded, as `browserRound` takes them. `refused` is the
 * caller's wording for `invalid_code`, as `exchangeCliCode` takes it.
 */
export async function deviceRound(
  target: string,
  opts: LoginOptions,
  pkce: Pkce,
  ask: { query?: string; purpose?: BrowserPurpose; refused?: CodeRefused } = {}
): Promise<Minted | undefined> {
  const purpose = ask.purpose ?? LOGIN_PURPOSE;
  const started = await deviceStart(target, opts, pkce, { query: ask.query, purpose });
  if (!started) return undefined;
  announceWaiting(started.expires_in);
  const wording = { refused: ask.refused ?? LOGIN_CODE_REFUSED, purpose };
  return pollUntilDecided(target, pkce.verifier, started, wording, opts.sleep ?? defaultSleep);
}

/**
 * Start a request and say where to approve it, without waiting for it.
 * `undefined` when the deployment has no device-code route.
 */
export async function deviceStart(
  target: string,
  opts: LoginOptions,
  pkce: Pkce,
  ask: { query?: string; purpose?: BrowserPurpose } = {},
  /**
   * Run between the start and the announcement — `--no-wait` records the
   * request here, so a failure to record it is reported before anyone is told
   * to approve a request nothing could collect.
   */
  beforeAnnounce?: (started: DeviceRequest) => void
): Promise<DeviceRequest | undefined> {
  const hostname = opts.hostname ?? osHostname();
  const approver = opts.approver?.trim() || undefined;
  const started = await startRequest(target, pkce, hostname, ask.query, approver);
  if (started) {
    beforeAnnounce?.(started);
    announce(target, started, ask.purpose ?? LOGIN_PURPOSE, approver);
  }
  return started;
}
