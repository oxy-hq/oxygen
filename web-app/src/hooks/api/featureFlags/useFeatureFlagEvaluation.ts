import { useQuery } from "@tanstack/react-query";
import { FeatureFlagsService } from "@/services/api/featureFlags";
import queryKeys from "../queryKey";

/** What each flag serves that org / person. Idle until an id is given. */
export const useFeatureFlagEvaluation = ({ orgId, userId }: { orgId?: string; userId?: string }) =>
  useQuery({
    queryKey: queryKeys.featureFlags.evaluate(orgId ?? "", userId ?? ""),
    queryFn: () => FeatureFlagsService.evaluate({ orgId, userId }),
    enabled: !!orgId || !!userId
  });
