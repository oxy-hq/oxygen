import { useQuery } from "@tanstack/react-query";
import { apiStatus } from "@/libs/apiError";
import type { ApiKeyActivityResponse } from "@/types/apiKey";
import type { TokenActivityEndpoints } from "./tokenEndpoints";

/** The most recent events the drawer asks for; the server caps `limit` at 500. */
export const API_KEY_ACTIVITY_LIMIT = 100;

/**
 * Events, 30-day usage and last use for one token, from whichever surface `endpoints` names.
 * Pass `enabled` so it fetches only while shown.
 */
const useApiKeyActivity = (endpoints: TokenActivityEndpoints, id: string, enabled = true) =>
  useQuery<ApiKeyActivityResponse, Error>({
    queryKey: endpoints.keys.activity(id),
    queryFn: () => endpoints.activity(id, API_KEY_ACTIVITY_LIMIT),
    enabled,
    refetchOnWindowFocus: false,
    // A 404 is an answer (the endpoint isn't deployed yet, or the key is gone), not a blip.
    retry: (failureCount, error) => apiStatus(error) !== 404 && failureCount < 2
  });

export default useApiKeyActivity;
