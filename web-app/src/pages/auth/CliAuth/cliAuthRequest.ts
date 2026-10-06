/**
 * What `oxyc login` asked for when it opened `/cli-auth`, read from the query string.
 *
 * - `pkce`: a current oxyc. The page trades the session for a single-use code and sends only
 *   that to the loopback, so no credential ever appears in a URL.
 * - `legacy`: an older oxyc that sends no `code_challenge` and expects the session token itself.
 * - `invalid`: nothing usable.
 */
export type CliAuthRequest =
  | { kind: "invalid"; reason: string }
  | { kind: "legacy"; port: string; state: string }
  | { kind: "pkce"; port: string; state: string; codeChallenge: string; hostname: string };

export type PkceRequest = Extract<CliAuthRequest, { kind: "pkce" }>;
export type LegacyRequest = Extract<CliAuthRequest, { kind: "legacy" }>;

const INVALID_HANDOFF = "Invalid CLI login request (missing or malformed port/state).";
const INVALID_PKCE = "This login link is incomplete. Run `oxyc login` again to get a new one.";

// base64url of a SHA-256: always 43 characters, which is the only length the server accepts
// (`clean_challenge`). Padding is tolerated, as it is there. Anything else is refused here, so a
// mangled link says "run `oxyc login` again" instead of failing at Authorize with a 400.
const CODE_CHALLENGE = /^[A-Za-z0-9_-]{43}={0,2}$/;
const MAX_HOSTNAME = 255;
// The server refuses a control character in the hostname: it ends up in the token's name.
// biome-ignore lint/suspicious/noControlCharactersInRegex: matching control characters is the point
const CONTROL_CHARACTER = /[\u0000-\u001f\u007f-\u009f]/;

/**
 * Decide which flow the page runs.
 *
 * The presence of `code_challenge` alone picks PKCE. A malformed one is `invalid`, never
 * `legacy`: falling back would hand the session token to a client that asked not to receive it.
 */
export const parseCliAuthRequest = (params: URLSearchParams): CliAuthRequest => {
  const port = params.get("port");
  const state = params.get("state");
  if (!port || !/^\d+$/.test(port) || !state) {
    return { kind: "invalid", reason: INVALID_HANDOFF };
  }
  if (!params.has("code_challenge")) return { kind: "legacy", port, state };

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
    return { kind: "invalid", reason: INVALID_PKCE };
  }
  return { kind: "pkce", port, state, codeChallenge, hostname };
};

/**
 * Where the browser hands off to oxyc. Only ever `http://127.0.0.1:<port>`, built from the
 * integer `port`, so this can't be turned into an open redirect that leaks the payload off-box.
 */
const loopback = (port: string, payload: string, state: string): string =>
  `http://127.0.0.1:${port}/callback?${payload}&state=${encodeURIComponent(state)}`;

/** The PKCE handoff: a single-use code that is useless without oxyc's `code_verifier`. */
export const codeCallbackUrl = (request: PkceRequest, code: string): string =>
  loopback(request.port, `code=${encodeURIComponent(code)}`, request.state);

/** The legacy handoff: the session token itself, for an oxyc that predates PKCE. */
export const tokenCallbackUrl = (request: LegacyRequest, token: string): string =>
  loopback(request.port, `token=${encodeURIComponent(token)}`, request.state);

/** This page's own URL, to come back to after signing in. Carries the PKCE params when present. */
export const returnToUrl = (origin: string, request: PkceRequest | LegacyRequest): string => {
  const base = `${origin}/cli-auth?port=${request.port}&state=${encodeURIComponent(request.state)}`;
  if (request.kind === "legacy") return base;
  return `${base}&code_challenge=${encodeURIComponent(
    request.codeChallenge
  )}&hostname=${encodeURIComponent(request.hostname)}`;
};

export const loginUrl = (returnTo: string): string =>
  `/login?return_to=${encodeURIComponent(returnTo)}`;
