import { useQuery } from "@tanstack/react-query";
import { apiStatus } from "@/libs/apiError";
import { CliAuthService } from "@/services/api/apiToken";
import type { CliServiceAccountMintRequest, ServiceAccountMintPreview } from "@/types/apiToken";
import queryKeys from "../queryKey";

/**
 * What approving a service-account token's mint would create, as the server would create it
 * (`POST /auth/cli/service-account/preview`): the approval page lays this out rather than its own
 * reading of the request. Read once, like the device-code request: the page approves what it
 * showed. A 4xx is the server refusing the mint, with the reason the approval would give, so it
 * is an answer, not a failure to retry.
 */
const useServiceAccountPreview = (hostname: string, mint: CliServiceAccountMintRequest | null) =>
  useQuery<ServiceAccountMintPreview, Error>({
    queryKey: queryKeys.cliDevice.serviceAccountPreview(hostname, mint),
    queryFn: () =>
      CliAuthService.previewServiceAccount(hostname, mint as CliServiceAccountMintRequest),
    enabled: mint !== null,
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    retry: (failures, error) => (apiStatus(error) ?? 500) >= 500 && failures < 2
  });

export default useServiceAccountPreview;
