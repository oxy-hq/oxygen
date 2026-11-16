import type { QueryKey } from "@tanstack/react-query";
import { UserTokenService } from "@/services/api/apiToken";
import type { ApiKeyActivityResponse, ExtendApiKeyRequest } from "@/types/apiKey";
import queryKeys from "../queryKey";

/** What an extend answers with, on any surface: enough to say what happened. */
export interface ExtendedToken {
  name: string;
  expires_at?: string | null;
}

/**
 * Where one token's activity is read from. All the Activity drawer needs, so a read-only surface
 * (an org's token inventory) can open the drawer without offering Extend.
 */
export interface TokenActivityEndpoints {
  activity: (id: string, limit: number) => Promise<ApiKeyActivityResponse>;
  keys: {
    activity: (id: string) => QueryKey;
  };
}

/**
 * Where one surface's tokens live. The Extend popover, the Activity drawer and the status badge
 * take a row plus one of these, so Account → Personal access tokens, an org's service accounts
 * and its read-only inventory share the components and differ only in routes.
 */
export interface TokenEndpoints extends TokenActivityEndpoints {
  extend: (id: string, request: ExtendApiKeyRequest) => Promise<ExtendedToken>;
  keys: {
    /** Every list a change to one token makes stale. */
    lists: readonly QueryKey[];
    activity: (id: string) => QueryKey;
  };
}

/** `/user/tokens`: the caller's own personal access tokens. Needs no workspace. */
export const USER_TOKEN_ENDPOINTS: TokenEndpoints = {
  extend: (id, request) => UserTokenService.extend(id, request),
  activity: (id, limit) => UserTokenService.activity(id, limit),
  keys: {
    // `apiKey.all` covers the inventory of every workspace the token reaches.
    lists: [queryKeys.userToken.list(), queryKeys.apiKey.all],
    activity: (id) => queryKeys.userToken.activity(id)
  }
};
