import { useQuery } from "@tanstack/react-query";
import { apiStatus } from "@/libs/apiError";
import { UserTokenService } from "@/services/api/apiToken";
import type { TokenOptions, UserTokenListResponse } from "@/types/apiToken";
import queryKeys from "../queryKey";

// A 404 is an answer (this server predates personal tokens), not a blip worth retrying.
const retryUnlessMissing = (failureCount: number, error: Error) =>
  apiStatus(error) !== 404 && failureCount < 2;

/** The caller's personal access tokens, newest first. The route returns no legacy API keys. */
export const useUserTokens = () =>
  useQuery<UserTokenListResponse, Error>({
    queryKey: queryKeys.userToken.list(),
    queryFn: UserTokenService.list,
    refetchOnWindowFocus: false,
    retry: retryUnlessMissing
  });

/**
 * The orgs, workspaces and standing the caller can put on a token. Fetched only while a dialog
 * that needs it is open.
 */
export const useTokenOptions = (enabled = true) =>
  useQuery<TokenOptions, Error>({
    queryKey: queryKeys.userToken.options(),
    queryFn: UserTokenService.options,
    enabled,
    refetchOnWindowFocus: false,
    retry: retryUnlessMissing
  });
