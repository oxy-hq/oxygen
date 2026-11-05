import { useMutation, useQueryClient } from "@tanstack/react-query";
import {
  type FeatureFlag,
  type FeatureFlagPatch,
  FeatureFlagsService
} from "@/services/api/featureFlags";
import queryKeys from "../queryKey";

interface UpdateInput extends FeatureFlagPatch {
  key: string;
}

export const useUpdateFeatureFlag = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ key, ...patch }: UpdateInput) => FeatureFlagsService.update(key, patch),
    onMutate: async ({ key, enabled }) => {
      // Only the kill switch is optimistic; rules come back from the server (it validates them).
      if (enabled === undefined) return { previous: undefined };
      await qc.cancelQueries({ queryKey: queryKeys.featureFlags.list() });
      const previous = qc.getQueryData<FeatureFlag[]>(queryKeys.featureFlags.list());
      if (previous) {
        qc.setQueryData<FeatureFlag[]>(
          queryKeys.featureFlags.list(),
          previous.map((flag) => (flag.key === key ? { ...flag, enabled } : flag))
        );
      }
      return { previous };
    },
    onError: (_err, _vars, context) => {
      if (context?.previous) {
        qc.setQueryData(queryKeys.featureFlags.list(), context.previous);
      }
    },
    onSettled: () => {
      qc.invalidateQueries({ queryKey: queryKeys.featureFlags.all });
      qc.invalidateQueries({ queryKey: queryKeys.featureFlags.orgs });
    }
  });
};
