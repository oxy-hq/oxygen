import type { Token } from "@/types/apiToken";
import { apiClient } from "./axios";

/**
 * How many tokens `GET /admin/standing-tokens` returns at most. The response does not say whether
 * it stopped there, so a list of exactly this many is read as cut short.
 */
export const STANDING_TOKEN_LIST_LIMIT = 500;

/**
 * Every personal API token that carries its owner's staff or partner standing
 * (`/api/admin/standing-tokens`), for staff holding `manage_platform_grants`. An owner manages
 * their own on `/user/tokens`; this is the shared view.
 */
export const StandingTokensService = {
  /** Newest first: every token that still works, then the newest expired and revoked ones. */
  async list(): Promise<Token[]> {
    const response = await apiClient.get<{ tokens: Token[] }>("/admin/standing-tokens");
    return response.data.tokens;
  },

  /** Answers the token as it now is. Revoking a revoked token changes nothing. */
  async revoke(id: string): Promise<Token> {
    const response = await apiClient.post<Token>(`/admin/standing-tokens/${id}/revoke`);
    return response.data;
  }
};
