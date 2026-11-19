/**
 * Wire types for Organization → API access: service accounts, trusted-access
 * (GitHub Actions OIDC) policies, the org token inventory and the org token
 * policy. All snake_case, as the server sends them. The token DTOs they are
 * built from (`Token`, `Grant`, `GrantInput`, `ExpiryInput`, …) are the
 * contract's shared ones, in `./apiToken`.
 */
import type { ExpiryInput, Grant, GrantInput, Token, TokenKind } from "./apiToken";

export type ServiceAccountRole = "member" | "admin";

export interface ServiceAccount {
  /** Also the `users.id` of the account. */
  id: string;
  org_id: string;
  /** A slug, unique within the org. */
  name: string;
  description: string | null;
  org_role: ServiceAccountRole;
  created_by: { id: string; label: string } | null;
  created_at: string;
  disabled_at: string | null;
  token_count: number;
  trust_policy_count: number;
}

export interface CreateServiceAccountRequest {
  name: string;
  description?: string;
  org_role?: ServiceAccountRole;
}

export interface UpdateServiceAccountRequest {
  description?: string | null;
  org_role?: ServiceAccountRole;
  disabled?: boolean;
}

export type CreateServiceAccountTokenRequest = {
  name: string;
  /** Omitted or empty: one org-wide grant at the account's role. */
  grants?: GrantInput[];
} & ExpiryInput;

export interface TrustPolicy {
  id: string;
  org_id: string;
  service_account_id: string;
  provider: "github_actions";
  /** `owner/repo`. Display only; the ids below are what is matched. */
  repository: string;
  repository_id: number;
  repository_owner_id: number;
  workflow_path: string;
  environment: string | null;
  ref_pattern: string | null;
  allow_self_hosted: boolean;
  grants: Grant[];
  created_by: { id: string; label: string } | null;
  created_at: string;
  last_used_at: string | null;
  disabled_at: string | null;
}

export interface CreateTrustPolicyRequest {
  repository: string;
  workflow_path: string;
  environment?: string | null;
  ref_pattern?: string | null;
  allow_self_hosted?: boolean;
  grants?: GrantInput[];
  /** Only when the server couldn't resolve them from `repository`. */
  repository_id?: number;
  repository_owner_id?: number;
}

/** `repository` is fixed once a policy exists; everything else can change. */
export interface UpdateTrustPolicyRequest {
  workflow_path?: string;
  environment?: string | null;
  ref_pattern?: string | null;
  allow_self_hosted?: boolean;
  grants?: GrantInput[];
  disabled?: boolean;
}

/** Where a registered issuer's signing keys come from. */
export type IssuerKeySource = "discovery" | "jwks_uri" | "inline";

/** An OIDC issuer whose tokens the org trusts. */
export interface OidcIssuer {
  id: string;
  org_id: string;
  name: string;
  /** The `iss` its tokens carry, without a trailing `/`. */
  issuer: string;
  key_source: IssuerKeySource;
  /** `discovery` only, when the document was not read from the issuer. */
  discovery_url: string | null;
  /** Where its keys are read from (`discovery` and `jwks_uri`). */
  jwks_uri: string | null;
  /** `inline` only: the `kid`s of the stored keys. */
  inline_key_ids: string[];
  /** Each token's `jti` may be exchanged once only. */
  jti_single_use: boolean;
  /** Which settings-form preset filled it in. Display only. */
  preset: string | null;
  /** Trust policies on it, across the org's service accounts. */
  policy_count: number;
  created_by: { id: string; label: string } | null;
  created_at: string;
}

export interface CreateOidcIssuerRequest {
  name: string;
  issuer: string;
  key_source: IssuerKeySource;
  discovery_url?: string;
  jwks_uri?: string;
  /** A JSON Web Key Set, public keys only. */
  jwks?: unknown;
  jti_single_use: boolean;
  preset: string;
}

/** A top-level claim of the token, and the value it must have: exact, or a prefix ending in one `*`. */
export interface ClaimCondition {
  claim: string;
  value: string;
}

/** Trusted access from a registered issuer: tokens whose claims meet the conditions act as the account. */
export interface IssuerTrustPolicy {
  id: string;
  org_id: string;
  service_account: { id: string; name: string };
  issuer: { id: string; name: string; issuer: string };
  /** Ordered by claim. */
  conditions: ClaimCondition[];
  grants: Grant[];
  created_by: { id: string; label: string } | null;
  created_at: string;
  last_used_at: string | null;
  disabled_at: string | null;
}

export interface CreateIssuerTrustPolicyRequest {
  issuer_id: string;
  conditions: ClaimCondition[];
  grants: GrantInput[];
}

/** The issuer and conditions are fixed; a policy for other tokens is another policy. */
export interface UpdateIssuerTrustPolicyRequest {
  grants?: GrantInput[];
  disabled?: boolean;
}

/** A row of the org inventory: a token, plus what it reaches *in this org*. */
export interface InventoryToken extends Token {
  grants_here: Grant[];
  /** Why the org's policy blocks it here (`max_lifetime | all_access_disallowed`), or `null`. */
  blocked_by_policy: string | null;
  /**
   * A non-legacy token with no expiry or more than 90 days left, in an org whose CI already uses
   * trusted access: a stored secret that may no longer be needed.
   */
  long_lived_while_trusted_access: boolean;
}

export interface InventoryFilters {
  kind?: TokenKind;
  /** The owner's id: a user id or a service account id. */
  owner?: string;
  workspace_id?: string;
}

export interface TokenPolicy {
  /** `null` = no limit. */
  max_lifetime_days: number | null;
  allow_all_access_tokens: boolean;
  require_environment_on_trust_policies: boolean;
}
