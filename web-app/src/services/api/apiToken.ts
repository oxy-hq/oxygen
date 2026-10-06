import type { ApiKeyActivityResponse } from "@/types/apiKey";
import type {
  CliAuthorizeRequest,
  CliAuthorizeResponse,
  CreateSandboxAgentTokenRequest,
  CreateTokenRequest,
  ExtendBody,
  Token,
  TokenOptions,
  TokenWithSecret,
  UpdateTokenRequest,
  UserTokenListResponse,
  WorkspaceTokenListResponse
} from "@/types/apiToken";
import { apiClient } from "./axios";

/**
 * The caller's own personal access tokens, and the sandbox agent tokens they minted. Every route
 * needs a browser session. A legacy API key is not a token: it is never listed here, and its id
 * answers 404 on these routes. A sandbox agent token is fixed once minted: `update`, `extend`
 * and `regenerate` answer 409 `sandbox_token_fixed` for one.
 */
export const UserTokenService = {
  async list(): Promise<UserTokenListResponse> {
    const response = await apiClient.get<UserTokenListResponse>("/user/tokens");
    return response.data;
  },

  /** One route mints both: a body with `kind: "sandbox_agent"` is a sandbox agent token. */
  async create(
    request: CreateTokenRequest | CreateSandboxAgentTokenRequest
  ): Promise<TokenWithSecret> {
    const response = await apiClient.post<TokenWithSecret>("/user/tokens", request);
    return response.data;
  },

  async get(id: string): Promise<Token> {
    const response = await apiClient.get<Token>(`/user/tokens/${id}`);
    return response.data;
  },

  /** `grants`, when sent, replaces the token's whole set. */
  async update(id: string, request: UpdateTokenRequest): Promise<Token> {
    const response = await apiClient.patch<Token>(`/user/tokens/${id}`, request);
    return response.data;
  },

  async extend(id: string, request: ExtendBody): Promise<Token> {
    const response = await apiClient.post<Token>(`/user/tokens/${id}/extend`, request);
    return response.data;
  },

  /** Same id and grants, new secret. The old secret stops working at once. */
  async regenerate(id: string): Promise<TokenWithSecret> {
    const response = await apiClient.post<TokenWithSecret>(`/user/tokens/${id}/regenerate`);
    return response.data;
  },

  async revoke(id: string): Promise<void> {
    await apiClient.delete(`/user/tokens/${id}`);
  },

  async activity(id: string, limit = 100): Promise<ApiKeyActivityResponse> {
    const response = await apiClient.get<ApiKeyActivityResponse>(`/user/tokens/${id}/activity`, {
      params: { limit }
    });
    return response.data;
  },

  async options(): Promise<TokenOptions> {
    const response = await apiClient.get<TokenOptions>("/user/token-options");
    return response.data;
  }
};

/** Every token that can reach one workspace, whoever owns it. Read-only, workspace admins. */
export const WorkspaceTokenService = {
  async list(workspaceId: string): Promise<WorkspaceTokenListResponse> {
    const response = await apiClient.get<WorkspaceTokenListResponse>(`/${workspaceId}/api-tokens`);
    return response.data;
  }
};

/**
 * The browser half of an oxyc PKCE flow: trade the session for a single-use code. With `mint`,
 * the code yields a sandbox agent token instead of a login.
 */
export const CliAuthService = {
  async authorize(request: CliAuthorizeRequest): Promise<CliAuthorizeResponse> {
    const response = await apiClient.post<CliAuthorizeResponse>("/auth/cli/authorize", request);
    return response.data;
  }
};
