/**
 * What oxyc asked for when it opened `/cli-auth`, read from the query string.
 *
 * - `pkce`: `oxyc login` from a current oxyc. The page trades the session for a single-use code
 *   and sends only that to the loopback, so no credential ever appears in a URL.
 * - `mint`: `oxyc tokens create --sandbox-agent`. The same PKCE handoff, with `kind` and what to
 *   mint beside it: the code oxyc exchanges yields that token instead of a login.
 * - `agent_mint`: `oxyc tokens create --agent`. The same handoff again, for an agent token: a
 *   personal token that reaches what its approver does, for hours.
 * - `service_account_mint`: `oxyc tokens create --org-service-account`. The same handoff, for a
 *   token of an organization's service account, the account created when it has none.
 * - `trust_policy_mint`: `oxyc init-ci`. The same handoff, for a trust policy that lets one
 *   GitHub Actions workflow publish one app as a service account. It mints no token.
 * - `legacy`: an older oxyc that sends no `code_challenge` and expects the session token itself.
 * - `invalid`: nothing usable. With a `title` it was a token request, and is refused as one.
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
    }
  | {
      kind: "agent_mint";
      port: string;
      state: string;
      codeChallenge: string;
      hostname: string;
      ask: AgentAsk;
    }
  | {
      kind: "service_account_mint";
      port: string;
      state: string;
      codeChallenge: string;
      hostname: string;
      ask: ServiceAccountAsk;
    }
  | {
      kind: "trust_policy_mint";
      port: string;
      state: string;
      codeChallenge: string;
      hostname: string;
      ask: TrustPolicyAsk;
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

/**
 * What oxyc asked for an agent token to be, as it arrived. As with `MintAsk`, nothing is
 * resolved or checked here: the page reads it against what the signed-in person holds.
 */
export interface AgentAsk {
  /** The `hours` param, or `null` when oxyc left the lifetime to the server's default. */
  hours: string | null;
  /** The `name` param, or `null` when oxyc sent none. */
  name: string | null;
  /** `standing=1`: carry the approver's staff or partner standing, if they hold any. */
  standing: boolean;
}

/**
 * What oxyc asked an organization's service-account token to be, as it arrived. Nothing is
 * resolved or checked here: the page asks the server what approving it would create.
 */
export interface ServiceAccountAsk {
  /** The organization, by slug or id. */
  org: string;
  /** The service account's name. */
  account: string;
  /** The `role` param, or `null` when oxyc left it to the default (member). */
  role: string | null;
  /** The `days` param, or `null` when oxyc left the lifetime to the server's default. */
  days: string | null;
  /** The `name` param, or `null` when oxyc sent none. */
  name: string | null;
}

/**
 * What `oxyc init-ci` asked a trust policy to be, as it arrived. Nothing is resolved or checked
 * here: the page asks the server what approving it would write.
 */
export interface TrustPolicyAsk {
  /** The organization, by slug or id. */
  org: string;
  account: string;
  /** `create=1`: create the account when the organization has none of that name. */
  create: boolean;
  /** The id of the app the policy may publish. */
  app: string;
  repository: string;
  workflow: string;
  /** `null` when oxyc sent none. */
  environment: string | null;
}

export type PkceRequest = Extract<CliAuthRequest, { kind: "pkce" }>;
type MintRequest = Extract<CliAuthRequest, { kind: "mint" }>;
type AgentMintRequest = Extract<CliAuthRequest, { kind: "agent_mint" }>;
type ServiceAccountMintRequest = Extract<CliAuthRequest, { kind: "service_account_mint" }>;
type TrustPolicyMintRequest = Extract<CliAuthRequest, { kind: "trust_policy_mint" }>;
export type LegacyRequest = Extract<CliAuthRequest, { kind: "legacy" }>;
/** Any token request: they share the session check, the handoff and the sheet. */
export type TokenRequest =
  | MintRequest
  | AgentMintRequest
  | ServiceAccountMintRequest
  | TrustPolicyMintRequest;

const INVALID_HANDOFF = "Invalid CLI login request (missing or malformed port/state).";
const INVALID_PKCE = "This login link is incomplete. Run `oxyc login` again to get a new one.";
const MINT_TITLE = "Token request failed";
const INVALID_MINT = "This link is incomplete. Run the oxyc command again to get a new one.";
const UNKNOWN_KIND =
  "This link asks for a kind of token this page can't approve. Update oxyc, then run the command again.";
const TWO_KINDS =
  "This link asks for two kinds of token at once, so nothing on it can be approved. Run the oxyc command again to get a new one.";

/**
 * The kinds of token `/cli-auth` mints. Anything else in `kind` is refused, never guessed at,
 * and so is a link that asks for two.
 */
const SANDBOX_AGENT = "sandbox_agent";
const AGENT = "agent";
const SERVICE_ACCOUNT = "service_account";
const TRUST_POLICY = "trust_policy";

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

/** `standing` as oxyc writes it: present as `1` when asked for, absent when not. */
const parseStanding = (raw: string | null): boolean | null => {
  if (raw === null || raw === "0" || raw === "false") return false;
  if (raw === "1" || raw === "true") return true;
  return null;
};

/** What oxyc asked an agent token to be, or `null` when a param is one no oxyc would send. */
const parseAgentAsk = (params: URLSearchParams): AgentAsk | null => {
  const hours = params.get("hours");
  const name = params.get("name");
  const standing = parseStanding(params.get("standing"));
  if (standing === null) return null;
  if ([hours ?? "", name ?? ""].some((value) => CONTROL_CHARACTER.test(value))) return null;
  return { hours, name, standing };
};

/** What an organization's service-account token was asked to be, or `null` when it can't be read. */
const parseServiceAccountAsk = (params: URLSearchParams): ServiceAccountAsk | null => {
  const org = (params.get("org") ?? "").trim();
  const account = (params.get("account") ?? "").trim();
  const role = params.get("role");
  const days = params.get("days");
  const name = params.get("name");
  if (!org || !account) return null;
  const values = [org, account, role ?? "", days ?? "", name ?? ""];
  if (values.some((value) => CONTROL_CHARACTER.test(value))) return null;
  return { org, account, role, days, name };
};

/** What `oxyc init-ci` asked a trust policy to be, or `null` when it can't be read. */
const parseTrustPolicyAsk = (params: URLSearchParams): TrustPolicyAsk | null => {
  const get = (name: string) => params.get(name);
  const [org, account, app, repository, workflow] = [
    "org",
    "account",
    "app",
    "repository",
    "workflow"
  ].map((name) => (get(name) ?? "").trim());
  if (!org || !account || !app || !repository || !workflow) return null;
  const ask: TrustPolicyAsk = {
    org,
    account,
    create: get("create") === "1",
    app,
    repository,
    workflow,
    environment: get("environment")
  };
  const values = Object.values(ask).filter((value): value is string => typeof value === "string");
  return values.some((value) => CONTROL_CHARACTER.test(value)) ? null : ask;
};

/**
 * The params each kind may carry beside the PKCE ones. A link with a param of another kind and
 * not of its own asks for two kinds. `org` and `account` are shared by the two kinds that name a
 * service account; `hours` by the two agent tokens; `name` by every kind that names a token.
 */
const KIND_PARAMS: Record<string, string[]> = {
  [SANDBOX_AGENT]: ["apps", "hours", "name"],
  [AGENT]: ["hours", "standing", "name"],
  [SERVICE_ACCOUNT]: ["org", "account", "role", "days", "name"],
  [TRUST_POLICY]: ["org", "account", "create", "app", "repository", "workflow", "environment"]
};

const EVERY_KIND_PARAM = new Set(Object.values(KIND_PARAMS).flat());

/**
 * A link that asks for two kinds: `kind` twice, or one kind with a param only another kind
 * takes. The tokens are far apart in what they reach, so the page picks none.
 */
const asksForTwoKinds = (params: URLSearchParams): boolean => {
  if (new Set(params.getAll("kind")).size > 1) return true;
  const kind = params.get("kind") ?? "";
  const own = KIND_PARAMS[kind];
  // A kind the page doesn't know is refused as one, below, whatever else the link carries.
  if (!own) return false;
  return [...EVERY_KIND_PARAM].some((param) => !own.includes(param) && params.has(param));
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
 *
 * `kind` is read exactly: `agent` and `sandbox_agent` are different tokens, and a link that
 * names both, or mixes one's params into the other, is `invalid` too.
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

  const read = readTokenAsk(params);
  return read.kind === "invalid" ? read : { ...read, port, state, codeChallenge, hostname };
};

/** What a token request asks for, or why it can't be read. */
export type TokenAsk =
  | { kind: "mint"; ask: MintAsk }
  | { kind: "agent_mint"; ask: AgentAsk }
  | { kind: "service_account_mint"; ask: ServiceAccountAsk }
  | { kind: "trust_policy_mint"; ask: TrustPolicyAsk }
  | { kind: "invalid"; reason: string; title: string };

/**
 * Read what a token request asks for from its `kind` and the params beside it: from the link
 * oxyc opened, or from a device-code request (`oxyc … --no-browser`), which stores the same
 * params. The rules are the link's, whichever way they arrived: one known kind, said once,
 * with only its own params.
 */
export const readTokenAsk = (params: URLSearchParams): TokenAsk => {
  const incomplete: TokenAsk = { kind: "invalid", reason: INVALID_MINT, title: MINT_TITLE };
  if (asksForTwoKinds(params)) return { kind: "invalid", reason: TWO_KINDS, title: MINT_TITLE };
  // One kind, said once: a repeated `kind` is a link no oxyc writes.
  if (params.getAll("kind").length > 1) return incomplete;
  switch (params.get("kind")) {
    case SANDBOX_AGENT: {
      const ask = parseMintAsk(params);
      return ask ? { kind: "mint", ask } : incomplete;
    }
    case AGENT: {
      const ask = parseAgentAsk(params);
      return ask ? { kind: "agent_mint", ask } : incomplete;
    }
    case SERVICE_ACCOUNT: {
      const ask = parseServiceAccountAsk(params);
      return ask ? { kind: "service_account_mint", ask } : incomplete;
    }
    case TRUST_POLICY: {
      const ask = parseTrustPolicyAsk(params);
      return ask ? { kind: "trust_policy_mint", ask } : incomplete;
    }
    default:
      return { kind: "invalid", reason: UNKNOWN_KIND, title: MINT_TITLE };
  }
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
export const codeCallbackUrl = (request: PkceRequest | TokenRequest, code: string): string =>
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

/** The agent params as oxyc sent them: `standing` only when it was asked for. */
const agentQuery = (ask: AgentAsk): string => {
  const params = [`kind=${AGENT}`];
  if (ask.hours !== null) params.push(`hours=${encodeURIComponent(ask.hours)}`);
  if (ask.standing) params.push("standing=1");
  if (ask.name !== null) params.push(`name=${encodeURIComponent(ask.name)}`);
  return params.join("&");
};

/** The service account params as oxyc sent them. */
const serviceAccountQuery = (ask: ServiceAccountAsk): string => {
  const params = [
    `kind=${SERVICE_ACCOUNT}`,
    `org=${encodeURIComponent(ask.org)}`,
    `account=${encodeURIComponent(ask.account)}`
  ];
  if (ask.role !== null) params.push(`role=${encodeURIComponent(ask.role)}`);
  if (ask.days !== null) params.push(`days=${encodeURIComponent(ask.days)}`);
  if (ask.name !== null) params.push(`name=${encodeURIComponent(ask.name)}`);
  return params.join("&");
};

/** The trust policy params as oxyc sent them. */
const trustPolicyQuery = (ask: TrustPolicyAsk): string => {
  const e = encodeURIComponent;
  const params = [
    `kind=${TRUST_POLICY}`,
    `org=${e(ask.org)}`,
    `account=${e(ask.account)}`,
    ...(ask.create ? ["create=1"] : []),
    `app=${e(ask.app)}`,
    `repository=${e(ask.repository)}`,
    `workflow=${e(ask.workflow)}`
  ];
  if (ask.environment !== null) params.push(`environment=${e(ask.environment)}`);
  return params.join("&");
};

/**
 * This page's own URL, to come back to after signing in. Carries the PKCE params when present,
 * and what a mint asked for, so the flow resumes as the one that was started.
 */
export const returnToUrl = (
  origin: string,
  request: PkceRequest | TokenRequest | LegacyRequest
): string => {
  const base = `${origin}/cli-auth?port=${request.port}&state=${encodeURIComponent(request.state)}`;
  if (request.kind === "legacy") return base;
  const pkce = `${base}&code_challenge=${encodeURIComponent(
    request.codeChallenge
  )}&hostname=${encodeURIComponent(request.hostname)}`;
  switch (request.kind) {
    case "mint":
      return `${pkce}&${mintQuery(request.ask)}`;
    case "agent_mint":
      return `${pkce}&${agentQuery(request.ask)}`;
    case "service_account_mint":
      return `${pkce}&${serviceAccountQuery(request.ask)}`;
    case "trust_policy_mint":
      return `${pkce}&${trustPolicyQuery(request.ask)}`;
    case "pkce":
      return pkce;
  }
};

export const loginUrl = (returnTo: string): string =>
  `/login?return_to=${encodeURIComponent(returnTo)}`;
