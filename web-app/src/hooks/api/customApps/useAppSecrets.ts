import { type QueryClient, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";

import { CustomAppsService } from "@/services/api/customApps";
import { PRODUCTION } from "@/types/appEnvironments";
import { errMessage } from "../errMessage";
import { isForbidden } from "../httpStatus";
import queryKeys from "../queryKey";

/**
 * The app's secrets in `environment` (production by default): what the build
 * that environment serves declares (`env` in `oxy-app.json`, plus every
 * function's `webhook.secretVar`), unioned with what is actually stored there.
 * Values are never in this payload.
 *
 * A 403 is an answer, not a fault — the caller may not open this app's
 * non-production environments — so it is not retried.
 */
export function useAppSecrets(id: string | undefined, environment: string = PRODUCTION) {
  return useQuery({
    queryKey: queryKeys.customApps.secrets(id ?? "", environment),
    queryFn: () => CustomAppsService.listSecrets(id as string, environment),
    enabled: !!id,
    retry: (failures, error) => !isForbidden(error) && failures < 3
  });
}

/**
 * The required keys `build` declares that `environment` has nothing for —
 * stored there, inherited, or shared from the workspace. Asked before a promote, publish or rollback, to warn (never
 * block) that the build would go live missing them. `[]` while loading, on a
 * failure, or with no build: a warning that cannot be computed is not shown.
 */
export function useMissingSecretsForBuild(
  appId: string,
  build: string | undefined,
  environment: string = PRODUCTION
) {
  const query = useQuery({
    queryKey: queryKeys.customApps.secretsForBuild(appId, environment, build ?? ""),
    queryFn: () => CustomAppsService.listSecrets(appId, environment, build),
    enabled: !!build,
    // Not retried: the confirm waits on this check, and a warning that cannot be
    // computed is not shown anyway — so retries would only hold a rollback during
    // an outage of the secrets list for the backoff's ~7 s.
    retry: false
  });
  const missing = (query.data?.entries ?? [])
    .filter(
      (e) =>
        e.required &&
        !e.is_set &&
        !e.inherits_production &&
        !e.inherits_staging &&
        !e.shared_from_workspace
    )
    .map((e) => e.key);
  return { missing, isLoading: query.isLoading };
}

/** Copy another environment's value of a key into `environment`, server-side. */
export function useCopyAppSecret(id: string, environment: string) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ key, from }: { key: string; from: string }) =>
      CustomAppsService.copySecret(id, key, environment, from),
    onSuccess: () => invalidateAll(queryClient, id)
  });
}

/** Every environment's view of the app: a write in one changes what another
 *  inherits (a staging value lends to a sandbox, a shared production one to
 *  both), so a write refetches them all. */
const invalidateAll = (queryClient: QueryClient, id: string) =>
  queryClient.invalidateQueries({ queryKey: queryKeys.customApps.secrets(id) });

/** Create or rotate one key in `environment`, then refetch so the row flips to
 *  Set. Resolves with which of the two the server did. */
export function useSetAppSecret(id: string, environment: string = PRODUCTION) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: ({ key, value }: { key: string; value: string }) =>
      CustomAppsService.setSecret(id, key, value, environment),
    onSuccess: () => invalidateAll(queryClient, id)
  });
}

/**
 * Create or rotate an app-scoped secret from the workspace's own settings,
 * rather than the staff console. Invalidates the project secrets list, which is
 * what that surface renders — an app secret is a project secret with a reserved
 * name, so it appears in the same table.
 */
export function useSetWorkspaceAppSecret(workspaceId: string) {
  const queryClient = useQueryClient();
  return useMutation({
    // Resolves with what the write did: the endpoint creates or rotates, and
    // says which in its status.
    mutationFn: ({ appId, key, value }: { appId: string; key: string; value: string }) =>
      CustomAppsService.setWorkspaceAppSecret(workspaceId, appId, key, value),
    // The hook owns the success toast as well, as `useCreateSecret` does: the
    // dialog's callers only close it, so each path says it exactly once. Rotating
    // a key the app already had replaces a live value; that is not "created".
    onSuccess: (write) => {
      toast.success(
        write === "created" ? "Secret created successfully" : "Secret updated successfully"
      );
      // The sharing view too: an app's own value may be the key it was missing.
      void queryClient.invalidateQueries({ queryKey: queryKeys.secret.shared(workspaceId) });
      return queryClient.invalidateQueries({ queryKey: queryKeys.secret.list(workspaceId) });
    },
    // The dialog calling this catches and only `console.error`s, on the
    // assumption — true of `useCreateSecret`, and it had to become true here —
    // that the hook owns the toast. Without this a rejected key (400), an app
    // outside the workspace (404), or a missing role (403) all leave the dialog
    // open with the value still in the field and nothing saying why.
    //
    // The server's message names the rule that was broken, so it beats a generic
    // string; `errMessage` falls back when there is not one.
    onError: (error) => toast.error(errMessage(error, "Failed to save app secret"))
  });
}

export function useDeleteAppSecret(id: string, environment: string = PRODUCTION) {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (key: string) => CustomAppsService.deleteSecret(id, key, environment),
    onSuccess: () => invalidateAll(queryClient, id)
  });
}
