/**
 * The calling token, asked of the deployment: what it is, and how to end it.
 *
 * `GET /api/auth/token` and `DELETE /api/auth/token` are the two token routes
 * any credential may call — everything under `/api/user/tokens` is
 * browser-session-only. They are also the only two that answer for a LEGACY
 * API KEY: `GET` describes one with `kind: "legacy_key"` so `whoami` can name
 * it, and `DELETE` refuses it with 409. Both are absent on a deployment that predates the
 * token redesign, where they answer 404, and both answer 404 under a session
 * too. Neither case is an error here: the caller asked a question about a
 * credential, and "there is nothing to say" is an answer.
 */

import { type ApiResponse, parseJson, request } from "../api/request.js";

export type TokenKind = "personal" | "legacy_key" | "service_account" | "ci" | "sandbox_agent";
export type RoleCeiling = "viewer" | "member" | "admin" | "owner";

/** One thing a token may reach. The wire shape, field for field. */
export interface Grant {
  id: string;
  kind: "workspace" | "app_publish" | "app_sandbox";
  org_id: string;
  org_name: string;
  /** `null` is every workspace in the org, future ones included. */
  workspace_id: string | null;
  workspace_name: string | null;
  /** `null` on an `app_publish` or `app_sandbox` grant. `owner` means no cap. */
  role_ceiling: RoleCeiling | null;
  app_id: string | null;
  app_name: string | null;
  /** On an `app_sandbox` grant only: with `app_slug`, the app as `<org>/<app>`. */
  org_slug?: string;
  /** On an `app_sandbox` grant only. */
  app_slug?: string;
  revoked_at: string | null;
}

/** An app a sandbox agent token was minted for, as `GET /api/auth/token` lists it. */
export interface SandboxTokenApp {
  id: string;
  org_slug: string;
  slug: string;
  name: string;
}

/** The server's `Token`. Fields this tool does not read are still tolerated. */
export interface Token {
  id: string;
  name: string;
  kind: TokenKind;
  display_prefix: string;
  last_four: string;
  all_access: boolean;
  platform: boolean;
  partner: boolean;
  grants: Grant[];
  expires_at: string | null;
  last_used_at: string | null;
  created_at: string;
  revoked_at: string | null;
  status: "active" | "expired" | "revoked";
  source: string;
  owner: { type: "user" | "service_account"; id: string; label: string };
  blocked_orgs: { org_id: string; org_name: string; reason: string }[];
  /**
   * `sandbox_agent` only, and only from `GET /api/auth/token`: who minted it.
   * `email` is `null` for a minter whose account has none.
   */
  minter?: { user_id: string; email: string | null };
  /** `sandbox_agent` only, and only from `GET /api/auth/token`: the apps it reaches. */
  apps?: SandboxTokenApp[];
}

export type Introspection =
  | { kind: "token"; token: Token }
  /** The credential is a browser session — there is no token row behind it. */
  | { kind: "session" }
  /** The route is missing (an older deployment) or did not answer usefully. */
  | { kind: "unavailable"; status: number };

/** Lookups about a credential should never hold a command up for long. */
const INTROSPECT_TIMEOUT_MS = 15_000;

/**
 * What the calling token is. Never throws — not on a status, and not on the
 * network either: every caller has already done the thing it came to do, and
 * this is the footnote.
 */
export async function introspectToken(target: string, bearer: string): Promise<Introspection> {
  let response: ApiResponse;
  try {
    response = await request({
      target,
      path: "/api/auth/token",
      method: "GET",
      bearer,
      timeoutMs: INTROSPECT_TIMEOUT_MS
    });
  } catch {
    return { kind: "unavailable", status: 0 };
  }
  const body = parseJson(response.body) as (Partial<Token> & { code?: string }) | undefined;
  if (response.status === 200 && body && typeof body.id === "string") {
    return { kind: "token", token: normalizeToken(body) };
  }
  if (response.status === 404 && body?.code === "no_token") return { kind: "session" };
  return { kind: "unavailable", status: response.status };
}

/** Fill the collections a lenient server might omit, so callers never guard. */
export function normalizeToken(raw: Partial<Token>): Token {
  return {
    ...(raw as Token),
    grants: Array.isArray(raw.grants) ? raw.grants : [],
    blocked_orgs: Array.isArray(raw.blocked_orgs) ? raw.blocked_orgs : [],
    // Left absent when the server sent none: only one kind carries the list.
    ...(Array.isArray(raw.apps) ? { apps: raw.apps.filter(isSandboxTokenApp) } : {})
  };
}

/** A row a caller can resolve an app against: an id and both slugs. */
function isSandboxTokenApp(row: unknown): row is SandboxTokenApp {
  const app = row as Partial<SandboxTokenApp> | null;
  return (
    typeof app?.id === "string" && typeof app.org_slug === "string" && typeof app.slug === "string"
  );
}

export type RevokeOutcome =
  /** 204 — the token is dead. */
  | "revoked"
  /** 409 `legacy_immutable` — a legacy API key; only its owner can end one, in the web app. */
  | "legacy"
  /** 404 — a session, or a deployment with no such route. Nothing was revoked. */
  | "unsupported"
  /** 401/403 — the server no longer accepts it, which is the goal anyway. */
  | "already_invalid"
  /** Anything else, the network included. */
  | "failed";

/**
 * Revoke the token that makes the call. BEST-EFFORT BY CONTRACT: it never
 * throws, because every caller is on its way out — a logout that failed
 * because the server was unreachable would leave the token cached, and a
 * command that failed because its cleanup did would turn a success into an
 * error for a token that expires on its own.
 */
export async function revokeCallingToken(
  target: string,
  bearer: string,
  timeoutMs = 10_000
): Promise<RevokeOutcome> {
  try {
    const response = await request({
      target,
      path: "/api/auth/token",
      method: "DELETE",
      bearer,
      timeoutMs
    });
    if (response.status >= 200 && response.status < 300) return "revoked";
    if (response.status === 409) return "legacy";
    if (response.status === 404 || response.status === 405) return "unsupported";
    if (response.status === 401 || response.status === 403) return "already_invalid";
    return "failed";
  } catch {
    return "failed";
  }
}

/** `2026-12-30 (in 89 days)`, `never`, or `expired 2026-01-02`. */
export function describeExpiry(expiresAt: string | null | undefined, now = Date.now()): string {
  if (!expiresAt) return "never";
  const at = Date.parse(expiresAt);
  if (Number.isNaN(at)) return expiresAt;
  const day = expiresAt.slice(0, 10);
  const ms = at - now;
  if (ms <= 0) return `expired ${day}`;
  const minutes = Math.round(ms / 60_000);
  if (minutes < 120) return `${expiresAt} (in ${minutes} min)`;
  const hours = Math.round(ms / 3_600_000);
  if (hours < 48) return `${expiresAt} (in ${hours} h)`;
  return `${day} (in ${Math.round(ms / 86_400_000)} days)`;
}

function describeGrant(grant: Grant): string {
  if (grant.kind === "app_publish") {
    return `publish ${grant.org_name} / ${grant.app_name ?? grant.app_id ?? "?"}`;
  }
  if (grant.kind === "app_sandbox") {
    return `own sandboxes of ${grant.org_name} / ${grant.app_name ?? grant.app_id ?? "?"}`;
  }
  const where = grant.workspace_name ?? grant.workspace_id ?? "every workspace";
  const cap =
    !grant.role_ceiling || grant.role_ceiling === "owner"
      ? "no cap"
      : `up to ${grant.role_ceiling}`;
  return `${grant.org_name} / ${where} — ${cap}`;
}

/**
 * What the token can reach, one line each.
 *
 * All-access says so and stops: listing an all-access token's orgs would be a
 * second answer that goes stale the day its owner joins another. A narrowed
 * token lists its live grants; a grant the org revoked is left out rather than
 * shown struck through, because what a caller needs is what works.
 */
export function describeReach(token: Token): string[] {
  const standing = [token.platform ? "platform" : "", token.partner ? "partner" : ""].filter(
    Boolean
  );
  const lines: string[] = [];
  if (token.all_access) {
    lines.push(
      standing.length > 0 ? `all access (+ ${standing.join(", ")} standing)` : "all access"
    );
  } else {
    const live = token.grants.filter((g) => !g.revoked_at);
    if (live.length === 0) lines.push("nothing — every grant has been revoked");
    for (const grant of live) lines.push(describeGrant(grant));
    if (standing.length > 0) lines.push(`+ ${standing.join(", ")} standing`);
  }
  for (const blocked of token.blocked_orgs) {
    lines.push(`blocked in ${blocked.org_name}: ${blocked.reason}`);
  }
  return lines;
}
