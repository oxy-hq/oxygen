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
 */
export type TokenKind = "personal" | "legacy_key" | "service_account" | "ci";

/** What a grant lets the token do in a workspace. `owner` means "no cap". */
export type RoleCeiling = "viewer" | "member" | "admin" | "owner";

export type TokenStatus = "active" | "expired" | "revoked";

export interface Grant {
  id: string;
  kind: "workspace" | "app_publish";
  org_id: string;
  org_name: string;
  /** `null` = every workspace in the org, including ones created later. */
  workspace_id: string | null;
  workspace_name: string | null;
  /** `null` for an `app_publish` grant. */
  role_ceiling: RoleCeiling | null;
  app_id: string | null;
  app_name: string | null;
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
  /** `ui | oxyc_login | oidc | legacy_backfill | legacy_endpoint | legacy_lazy`. */
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

/** `GET /user/token-options`: what the create dialog can offer this caller. */
export interface TokenOptions {
  orgs: TokenOptionOrg[];
  can_platform: boolean;
  can_partner: boolean;
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

/** `POST /auth/cli/authorize`, the browser half of `oxyc login` (PKCE). */
export interface CliAuthorizeRequest {
  code_challenge: string;
  hostname: string;
}

export interface CliAuthorizeResponse {
  code: string;
}
