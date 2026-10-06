import type { ExtendApiKeyRequest } from "./apiKey";

/**
 * The shared token DTOs of the tokens HTTP contract (`Token`, `Grant`, `GrantInput`, `ExpiryInput`,
 * `ExtendBody` and the unions), plus the wire shapes of `/api/user/tokens` and the workspace token
 * inventory. Organization → API access adds its own in `orgApiAccess.ts` and imports these.
 */

/**
 * `legacy_key` is a legacy API key, not a token. It shares this union because two routes describe
 * one in the token shape: an org's inventory (`GET /orgs/{id}/tokens`) and `GET /auth/token`.
 * `/user/tokens` and a workspace's token inventory never return it.
 *
 * `sandbox_agent` is the token an AI agent holds while it builds custom apps (`oxy_sbx_…`). It
 * reaches the sandboxes of the apps it names and nothing else, lives hours, and is never
 * changed after it is minted: `/user/tokens` lists it beside the caller's personal tokens.
 */
export type TokenKind = "personal" | "legacy_key" | "service_account" | "ci" | "sandbox_agent";

/** What a grant lets the token do in a workspace. `owner` means "no cap". */
export type RoleCeiling = "viewer" | "member" | "admin" | "owner";

export type TokenStatus = "active" | "expired" | "revoked";

export interface Grant {
  id: string;
  /** `app_sandbox`: one app a sandbox agent token may build sandboxes of. */
  kind: "workspace" | "app_publish" | "app_sandbox";
  org_id: string;
  org_name: string;
  /**
   * `null` = every workspace in the org, including ones created later. Only for a `workspace`
   * grant: an app grant has no workspace, and its `null` means nothing of the kind.
   */
  workspace_id: string | null;
  workspace_name: string | null;
  /** `null` for an `app_publish` or `app_sandbox` grant. */
  role_ceiling: RoleCeiling | null;
  app_id: string | null;
  app_name: string | null;
  /**
   * On an `app_sandbox` grant only: with `app_slug`, the `<org>/<app>` reference oxyc names the
   * app by. Absent on every other kind and from a server that predates it, and empty when the
   * org is gone.
   */
  org_slug?: string;
  /** On an `app_sandbox` grant only. Empty when the app is gone. */
  app_slug?: string;
  /** Set when the org took the grant away. The token keeps its other grants. */
  revoked_at: string | null;
}

export interface TokenOwner {
  type: "user" | "service_account";
  id: string;
  label: string;
}

export interface BlockedOrg {
  org_id: string;
  org_name: string;
  reason: string;
}

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
  status: TokenStatus;
  /**
   * `ui | oxyc_login | oidc | legacy_backfill | legacy_endpoint | legacy_lazy`. A sandbox agent
   * token is `ui` or `oxyc`.
   */
  source: string;
  owner: TokenOwner;
  /** Orgs whose policy refuses this token. Empty until Phase 5. */
  blocked_orgs: BlockedOrg[];
}

/**
 * What the shared row pieces (status badge, Extend, Activity) read: one legacy API key, or one
 * token. A legacy `ApiKey` already has this shape; a `Token` gets there through `toTokenSummary`.
 */
export interface TokenSummary {
  id: string;
  name: string;
  expires_at?: string | null;
  /** False once revoked. Expiry is judged from `expires_at`, so it needs no refetch to lapse. */
  is_active: boolean;
  masked_key?: string;
  /**
   * `legacy_key` or absent is a legacy API key: the legacy `/api-keys` routes list nothing else
   * and send no `kind`. Anything else is a token. The copy the shared pieces show follows this.
   */
  kind?: TokenKind;
}

/** One grant in a create or update body. `org_id` is implied on an org's own routes. */
export interface GrantInput {
  kind?: "workspace" | "app_publish";
  org_id?: string;
  workspace_id?: string | null;
  role_ceiling?: RoleCeiling;
  app_id?: string;
}

/** Omitted entirely, the server gives the token 90 days. */
export type ExpiryInput = { expires_in_days: number } | { expires_at: string | null };

export interface TokenAccessInput {
  all_access?: boolean;
  platform?: boolean;
  partner?: boolean;
  /** On update this replaces the whole set. */
  grants?: GrantInput[];
}

export type CreateTokenRequest = { name: string } & TokenAccessInput & ExpiryInput;

/**
 * The `POST /user/tokens` body that mints a sandbox agent token. It carries none of a personal
 * token's fields: sending one of them with this `kind` is a 400.
 */
export interface CreateSandboxAgentTokenRequest {
  name: string;
  kind: "sandbox_agent";
  /** App ids: 1 to `sandbox_agent.max_apps`, no repeats. */
  apps: string[];
  /** 1 to `sandbox_agent.max_hours`. Omitted, the server gives `default_hours`. */
  expires_in_hours?: number;
}

export type UpdateTokenRequest = { name?: string } & TokenAccessInput;

/** `{ days }` counts from the later of now and the current expiry; `expires_at: null` is no expiry. */
export type ExtendBody = ExtendApiKeyRequest;

/** Create and regenerate both answer this. `secret` is never sent again. */
export interface TokenWithSecret {
  token: Token;
  secret: string;
}

export interface UserTokenListResponse {
  tokens: Token[];
}

export interface TokenOptionWorkspace {
  workspace_id: string;
  name: string;
  /** The caller's own role there: a grant can't lift the token above it. */
  role: RoleCeiling;
}

export interface TokenOrgPolicy {
  max_lifetime_days: number | null;
  allow_all_access_tokens: boolean;
}

export interface TokenOptionOrg {
  org_id: string;
  org_name: string;
  org_slug: string;
  role: "owner" | "admin" | "member";
  /** `partner`: reached through a partner grant, not a membership. */
  via: "member" | "partner";
  workspaces: TokenOptionWorkspace[];
  policy: TokenOrgPolicy;
}

/** The server's limits on a sandbox agent token, so the dialog never offers what it refuses. */
export interface SandboxAgentLimits {
  default_hours: number;
  max_hours: number;
  max_apps: number;
}

/** One custom app the caller may mint a sandbox agent token for. */
export interface SandboxApp {
  id: string;
  org_id: string;
  org_slug: string;
  org_name: string;
  slug: string;
  name: string;
}

/** `GET /user/token-options`: what the create dialog can offer this caller. */
export interface TokenOptions {
  orgs: TokenOptionOrg[];
  can_platform: boolean;
  can_partner: boolean;
  /** Absent from a server that predates sandbox agent tokens. */
  sandbox_agent?: SandboxAgentLimits;
  /**
   * The apps the caller may name on a sandbox agent token: empty for anyone who is not staff
   * with the reach to build them, and absent from a server that predates the kind. Either way
   * the type is not offered.
   */
  sandbox_apps?: SandboxApp[];
}

/** One row of `GET /{workspaceId}/api-tokens`: a token that can reach this workspace. */
export type WorkspaceTokenRow = Pick<
  Token,
  | "id"
  | "name"
  | "kind"
  | "display_prefix"
  | "all_access"
  | "expires_at"
  | "last_used_at"
  | "status"
  | "owner"
> & { role_ceiling_here: RoleCeiling };

export interface WorkspaceTokenListResponse {
  tokens: WorkspaceTokenRow[];
}

/**
 * What `oxyc tokens create --sandbox-agent` asks the browser to approve. `apps` are ids: the page
 * resolves the `<org>/<app>` slugs oxyc sent before it asks.
 */
export interface CliMintRequest {
  kind: "sandbox_agent";
  apps: string[];
  expires_in_hours: number;
  name: string;
}

/**
 * `POST /auth/cli/authorize`, the browser half of an oxyc PKCE flow. With no `mint` it is
 * `oxyc login`; with one, the code oxyc exchanges yields that token instead of a login.
 */
export interface CliAuthorizeRequest {
  code_challenge: string;
  hostname: string;
  mint?: CliMintRequest;
}

export interface CliAuthorizeResponse {
  code: string;
}
