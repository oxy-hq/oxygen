import type { ApiKeyActivityResponse } from "@/types/apiKey";
import type { ExtendBody, Token, TokenWithSecret } from "@/types/apiToken";
import type {
  CreateServiceAccountRequest,
  CreateServiceAccountTokenRequest,
  CreateTrustPolicyRequest,
  InventoryFilters,
  InventoryToken,
  ServiceAccount,
  TokenPolicy,
  TrustPolicy,
  UpdateServiceAccountRequest,
  UpdateTrustPolicyRequest
} from "@/types/orgApiAccess";
import { apiClient } from "./axios";

const accounts = (orgId: string) => `/orgs/${orgId}/service-accounts`;
const account = (orgId: string, saId: string) => `${accounts(orgId)}/${saId}`;

/**
 * Org service accounts and the `oxy_sat_` tokens they hold. Org admin only;
 * every mutation also needs a browser session (a token answers 403
 * `session_required`).
 */
export class ServiceAccountService {
  static async list(orgId: string): Promise<ServiceAccount[]> {
    const response = await apiClient.get<{ service_accounts: ServiceAccount[] }>(accounts(orgId));
    return response.data.service_accounts ?? [];
  }

  static async get(orgId: string, saId: string): Promise<ServiceAccount> {
    const response = await apiClient.get<ServiceAccount>(account(orgId, saId));
    return response.data;
  }

  static async create(orgId: string, body: CreateServiceAccountRequest): Promise<ServiceAccount> {
    const response = await apiClient.post<ServiceAccount>(accounts(orgId), body);
    return response.data;
  }

  static async update(
    orgId: string,
    saId: string,
    body: UpdateServiceAccountRequest
  ): Promise<ServiceAccount> {
    const response = await apiClient.patch<ServiceAccount>(account(orgId, saId), body);
    return response.data;
  }

  /** Also revokes the account's tokens and disables its trusted-access policies. */
  static async remove(orgId: string, saId: string): Promise<void> {
    await apiClient.delete(account(orgId, saId));
  }

  static async listTokens(orgId: string, saId: string): Promise<Token[]> {
    const response = await apiClient.get<{ tokens: Token[] }>(`${account(orgId, saId)}/tokens`);
    return response.data.tokens ?? [];
  }

  static async createToken(
    orgId: string,
    saId: string,
    body: CreateServiceAccountTokenRequest
  ): Promise<TokenWithSecret> {
    const response = await apiClient.post<TokenWithSecret>(`${account(orgId, saId)}/tokens`, body);
    return response.data;
  }

  static async extendToken(
    orgId: string,
    saId: string,
    tokenId: string,
    body: ExtendBody
  ): Promise<Token> {
    const response = await apiClient.post<Token>(
      `${account(orgId, saId)}/tokens/${tokenId}/extend`,
      body
    );
    return response.data;
  }

  /** Same id and grants; the old secret stops working at once. */
  static async regenerateToken(
    orgId: string,
    saId: string,
    tokenId: string
  ): Promise<TokenWithSecret> {
    const response = await apiClient.post<TokenWithSecret>(
      `${account(orgId, saId)}/tokens/${tokenId}/regenerate`
    );
    return response.data;
  }

  static async revokeToken(orgId: string, saId: string, tokenId: string): Promise<void> {
    await apiClient.delete(`${account(orgId, saId)}/tokens/${tokenId}`);
  }

  static async tokenActivity(
    orgId: string,
    saId: string,
    tokenId: string,
    limit: number
  ): Promise<ApiKeyActivityResponse> {
    const response = await apiClient.get<ApiKeyActivityResponse>(
      `${account(orgId, saId)}/tokens/${tokenId}/activity`,
      { params: { limit } }
    );
    return response.data;
  }
}

/** Trusted access: which GitHub Actions runs may act as a service account. */
export class TrustPolicyService {
  static async list(orgId: string, saId: string): Promise<TrustPolicy[]> {
    const response = await apiClient.get<{ trust_policies: TrustPolicy[] }>(
      `${account(orgId, saId)}/trust-policies`
    );
    return response.data.trust_policies ?? [];
  }

  static async create(
    orgId: string,
    saId: string,
    body: CreateTrustPolicyRequest
  ): Promise<TrustPolicy> {
    const response = await apiClient.post<TrustPolicy>(
      `${account(orgId, saId)}/trust-policies`,
      body
    );
    return response.data;
  }

  static async update(
    orgId: string,
    saId: string,
    policyId: string,
    body: UpdateTrustPolicyRequest
  ): Promise<TrustPolicy> {
    const response = await apiClient.patch<TrustPolicy>(
      `${account(orgId, saId)}/trust-policies/${policyId}`,
      body
    );
    return response.data;
  }

  static async remove(orgId: string, saId: string, policyId: string): Promise<void> {
    await apiClient.delete(`${account(orgId, saId)}/trust-policies/${policyId}`);
  }
}

/** Every token that reaches the org, whoever owns it. */
export class OrgTokenService {
  static async list(orgId: string, filters: InventoryFilters = {}): Promise<InventoryToken[]> {
    const response = await apiClient.get<{ tokens: InventoryToken[] }>(`/orgs/${orgId}/tokens`, {
      params: inventoryParams(filters)
    });
    return response.data.tokens ?? [];
  }

  /** Limited server-side to events that happened in this org. */
  static async activity(
    orgId: string,
    tokenId: string,
    limit: number
  ): Promise<ApiKeyActivityResponse> {
    const response = await apiClient.get<ApiKeyActivityResponse>(
      `/orgs/${orgId}/tokens/${tokenId}/activity`,
      { params: { limit } }
    );
    return response.data;
  }

  /** Ends a personal token's reach into this org only; its owner is emailed. */
  static async revokeGrant(orgId: string, tokenId: string): Promise<void> {
    await apiClient.post(`/orgs/${orgId}/tokens/${tokenId}/revoke-grant`);
  }
}

/** Only the filters that are set, so an empty one is never sent as `kind=`. */
export function inventoryParams(filters: InventoryFilters): Record<string, string> {
  const params: Record<string, string> = {};
  if (filters.kind) params.kind = filters.kind;
  if (filters.owner) params.owner = filters.owner;
  if (filters.workspace_id) params.workspace_id = filters.workspace_id;
  return params;
}

export class TokenPolicyService {
  static async get(orgId: string): Promise<TokenPolicy> {
    const response = await apiClient.get<TokenPolicy>(`/orgs/${orgId}/token-policy`);
    return response.data;
  }

  static async put(orgId: string, policy: TokenPolicy): Promise<TokenPolicy> {
    const response = await apiClient.put<TokenPolicy>(`/orgs/${orgId}/token-policy`, policy);
    return response.data;
  }
}
