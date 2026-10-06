/**
 * What oxyc asked for when it opened `/cli-auth`, read from the query string.
 *
 * - `pkce`: `oxyc login` from a current oxyc. The page trades the session for a single-use code
 *   and sends only that to the loopback, so no credential ever appears in a URL.
 * - `mint`: `oxyc tokens create --sandbox-agent`. The same PKCE handoff, with `kind` and what to
 *   mint beside it: the code oxyc exchanges yields that token instead of a login.
 * - `legacy`: an older oxyc that sends no `code_challenge` and expects the session token itself.
 * - `invalid`: nothing usable.
 */
export type CliAuthRequest =
  | { kind: "invalid"; reason: string; title?: string }
  | { kind: "legacy"; port: string; state: string }
  | { kind: "pkce"; port: string; state: string; codeChallenge: string; hostname: string }
  | {
      kind: "mint";
      port: string;
      state: string;
      codeChallenge: string;
      hostname: string;
      ask: MintAsk;
    };

/**
 * What oxyc asked to have minted, as it arrived. Nothing here is resolved or checked: the page
 * does that against what the signed-in person may mint, and shows whatever doesn't hold. Kept as
 * sent, so the request survives a trip through login unchanged.
 */
export interface MintAsk {
  /** `<org>/<app>` references, in the order given, each once. */
  apps: string[];
  /** The `hours` param, or `null` when oxyc left the lifetime to the server's default. */
  hours: string | null;
  /** The `name` param, or `null` when oxyc sent none. */
  name: string | null;
}

export type PkceRequest = Extract<CliAuthRequest, { kind: "pkce" }>;
export type MintRequest = Extract<CliAuthRequest, { kind: "mint" }>;
export type LegacyRequest = Extract<CliAuthRequest, { kind: "legacy" }>;

const INVALID_HANDOFF = "Invalid CLI login request (missing or malformed port/state).";
const INVALID_PKCE = "This login link is incomplete. Run `oxyc login` again to get a new one.";
const MINT_TITLE = "Token request failed";
const INVALID_MINT = "This link is incomplete. Run the oxyc command again to get a new one.";
const UNKNOWN_KIND =
  "This link asks for a kind of token this page can't approve. Update oxyc, then run the command again.";

/** The one kind of token `/cli-auth` mints. Anything else in `kind` is refused, never guessed at. */
const SANDBOX_AGENT = "sandbox_agent";

// base64url of a SHA-256: always 43 characters, which is the only length the server accepts
// (`clean_challenge`). Padding is tolerated, as it is there. Anything else is refused here, so a
// mangled link says "run `oxyc login` again" instead of failing at Authorize with a 400.
const CODE_CHALLENGE = /^[A-Za-z0-9_-]{43}={0,2}$/;
const MAX_HOSTNAME = 255;
// The server refuses a control character in the hostname: it ends up in the token's name.
// biome-ignore lint/suspicious/noControlCharactersInRegex: matching control characters is the point
const CONTROL_CHARACTER = /[\u0000-\u001f\u007f-\u009f]/;

/** What oxyc asked to mint, or `null` when a param carries something no oxyc would send. */
const parseMintAsk = (params: URLSearchParams): MintAsk | null => {
  const apps = params.get("apps") ?? "";
  const hours = params.get("hours");
  const name = params.get("name");
  if ([apps, hours ?? "", name ?? ""].some((value) => CONTROL_CHARACTER.test(value))) return null;
  const refs = apps
    .split(",")
    .map((ref) => ref.trim())
    .filter(Boolean);
  return { apps: [...new Set(refs)], hours, name };
};

/**
 * Decide which flow the page runs.
 *
 * The presence of `code_challenge` alone picks PKCE. A malformed one is `invalid`, never
 * `legacy`: falling back would hand the session token to a client that asked not to receive it.
 *
 * The presence of `kind` picks a mint, and a link with no `kind` is read exactly as before. A
 * `kind` this page doesn't know is `invalid`, never a login: falling back would have the person
 * approve an all-access login for a client that asked for something narrower. For the same
 * reason a mint is PKCE-only, and one with no `code_challenge` never runs the token handoff.
 */
export const parseCliAuthRequest = (params: URLSearchParams): CliAuthRequest => {
  const port = params.get("port");
  const state = params.get("state");
  if (!port || !/^\d+$/.test(port) || !state) {
    return { kind: "invalid", reason: INVALID_HANDOFF };
  }
  const asksToMint = params.has("kind");
  const incomplete: CliAuthRequest = asksToMint
    ? { kind: "invalid", reason: INVALID_MINT, title: MINT_TITLE }
    : { kind: "invalid", reason: INVALID_PKCE };
  if (!params.has("code_challenge")) {
    return asksToMint ? incomplete : { kind: "legacy", port, state };
  }

  const codeChallenge = params.get("code_challenge") ?? "";
  // The person confirms by hostname, so a request without one has nothing to confirm.
  const hostname = (params.get("hostname") ?? "").trim();
  if (
    !CODE_CHALLENGE.test(codeChallenge) ||
    !hostname ||
    // oxlint-disable-next-line typescript/no-misused-spread -- code points on purpose: the server's limit counts chars
    [...hostname].length > MAX_HOSTNAME ||
    CONTROL_CHARACTER.test(hostname)
  ) {
    return incomplete;
  }
  if (!asksToMint) return { kind: "pkce", port, state, codeChallenge, hostname };

  if (params.get("kind") !== SANDBOX_AGENT) {
    return { kind: "invalid", reason: UNKNOWN_KIND, title: MINT_TITLE };
  }
  const ask = parseMintAsk(params);
  return ask ? { kind: "mint", port, state, codeChallenge, hostname, ask } : incomplete;
};

/**
 * Where the browser hands off to oxyc. Only ever `http://127.0.0.1:<port>`, built from the
 * integer `port`, so this can't be turned into an open redirect that leaks the payload off-box.
 */
const loopback = (port: string, payload: string, state: string): string =>
  `http://127.0.0.1:${port}/callback?${payload}&state=${encodeURIComponent(state)}`;

/**
 * The PKCE handoff: a single-use code that is useless without oxyc's `code_verifier`. A login
 * and a mint hand off alike: what the code yields was settled when it was issued.
 */
export const codeCallbackUrl = (request: PkceRequest | MintRequest, code: string): string =>
  loopback(request.port, `code=${encodeURIComponent(code)}`, request.state);

/** The legacy handoff: the session token itself, for an oxyc that predates PKCE. */
export const tokenCallbackUrl = (request: LegacyRequest, token: string): string =>
  loopback(request.port, `token=${encodeURIComponent(token)}`, request.state);

/** The mint params as oxyc sent them. Without these a mint would come back from login a login. */
const mintQuery = (ask: MintAsk): string => {
  const params = [`kind=${SANDBOX_AGENT}`, `apps=${encodeURIComponent(ask.apps.join(","))}`];
  if (ask.hours !== null) params.push(`hours=${encodeURIComponent(ask.hours)}`);
  if (ask.name !== null) params.push(`name=${encodeURIComponent(ask.name)}`);
  return params.join("&");
};

/**
 * This page's own URL, to come back to after signing in. Carries the PKCE params when present,
 * and what a mint asked for, so the flow resumes as the one that was started.
 */
export const returnToUrl = (
  origin: string,
  request: PkceRequest | MintRequest | LegacyRequest
): string => {
  const base = `${origin}/cli-auth?port=${request.port}&state=${encodeURIComponent(request.state)}`;
  if (request.kind === "legacy") return base;
  const pkce = `${base}&code_challenge=${encodeURIComponent(
    request.codeChallenge
  )}&hostname=${encodeURIComponent(request.hostname)}`;
  return request.kind === "mint" ? `${pkce}&${mintQuery(request.ask)}` : pkce;
};

export const loginUrl = (returnTo: string): string =>
  `/login?return_to=${encodeURIComponent(returnTo)}`;
