import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { apiErrorMessage } from "@/libs/apiError";
import { SecretService } from "@/services/secretService";
import type { SharedSecret, SharedSecretsResponse } from "@/types/sharedSecrets";
import queryKeys from "../queryKey";

/**
 * Which apps each workspace secret is shared with, and the keys each app of the
 * workspace still lacks. Workspace-admin only, like the secrets list beside it.
 */
export const useSharedSecrets = (enabled = true) => {
  const { project } = useCurrentProjectBranch();
  return useQuery<SharedSecretsResponse, Error>({
    queryKey: queryKeys.secret.shared(project.id),
    queryFn: () => SecretService.listShared(project.id),
    enabled
  });
};

/**
 * Share one workspace secret with exactly `appIds`. Refetches the sharing view
 * and every app's secrets view (a shared key stops reading as missing there).
 */
export const useShareSecret = ({ toastOnSuccess = true }: { toastOnSuccess?: boolean } = {}) => {
  const queryClient = useQueryClient();
  const { project } = useCurrentProjectBranch();
  return useMutation<SharedSecret, Error, { secretId: string; name: string; appIds: string[] }>({
    mutationFn: ({ secretId, appIds }) => SecretService.shareWithApps(project.id, secretId, appIds),
    onSuccess: (_data, { name, appIds }) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.secret.shared(project.id) });
      queryClient.invalidateQueries({ queryKey: queryKeys.customApps.everyAppsSecrets() });
      if (toastOnSuccess) {
        toast.success(
          appIds.length === 0
            ? `${name} is no longer shared with any app`
            : `${name} is shared with ${appIds.length} app${appIds.length === 1 ? "" : "s"}`
        );
      }
    },
    onError: (error, { name }) => {
      toast.error(`Couldn't change which apps can read ${name}`, {
        description: apiErrorMessage(error, error.message)
      });
    }
  });
};
