import type { Token } from "@/types/apiToken";
import { apiClient } from "./axios";

/**
 * How many tokens `GET /admin/sandbox-agent-tokens` returns at most (`LIST_LIMIT` in
 * `user_tokens/sandbox_staff.rs`). The response does not say whether it stopped there, so a list
 * of exactly this many is read as cut short.
 */
export const SANDBOX_AGENT_TOKEN_LIST_LIMIT = 500;

/**
 * Every staff member's sandbox agent tokens (`/api/admin/sandbox-agent-tokens`), for staff holding
 * `operate_platform`. A minter manages their own on `/user/tokens`; this is the shared view.
 */
export const SandboxAgentTokensService = {
  /** Newest first, expired and revoked ones included, narrowed to the orgs the caller reaches. */
  async list(): Promise<Token[]> {
    const response = await apiClient.get<{ tokens: Token[] }>("/admin/sandbox-agent-tokens");
    return response.data.tokens;
  },

  /** Answers the token as it now is. Revoking a revoked token changes nothing. */
  async revoke(id: string): Promise<Token> {
    const response = await apiClient.post<Token>(`/admin/sandbox-agent-tokens/${id}/revoke`);
    return response.data;
  }
};
