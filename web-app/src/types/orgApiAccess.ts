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
