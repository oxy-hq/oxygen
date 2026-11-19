import { useQuery } from "@tanstack/react-query";
import { apiStatus } from "@/libs/apiError";
import { CliAuthService } from "@/services/api/apiToken";
import type { CliTrustPolicyMintRequest, TrustPolicyMintPreview } from "@/types/apiToken";
import queryKeys from "../queryKey";

/**
 * What approving a trust policy (`oxyc init-ci`) would write, as the server would write it
 * (`POST /auth/cli/trust-policy/preview`): the approval page lays this out rather than its own
 * reading of the request. Read once; a 4xx is the server refusing the request, with the reason
 * the approval would give, so it is an answer, not a failure to retry.
 */
const useTrustPolicyPreview = (mint: CliTrustPolicyMintRequest | null) =>
  useQuery<TrustPolicyMintPreview, Error>({
    queryKey: queryKeys.cliDevice.trustPolicyPreview(mint),
    queryFn: () => CliAuthService.previewTrustPolicy(mint as CliTrustPolicyMintRequest),
    enabled: mint !== null,
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    retry: (failures, error) => (apiStatus(error) ?? 500) >= 500 && failures < 2
  });

export default useTrustPolicyPreview;
