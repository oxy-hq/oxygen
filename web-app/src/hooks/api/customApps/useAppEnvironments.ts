import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { settling } from "@/pages/admin/AdminCustomApps/components/AppDetail/components/Environments/utils";
import { CustomAppsService } from "@/services/api/customApps";
import { errMessage } from "../errMessage";
import { isForbidden } from "../httpStatus";
import queryKeys from "../queryKey";

/**
 * The app's environments — production, staging, and every dev sandbox
 * (`GET /customer-apps/{id}/environments`).
 *
 * A deleting sandbox stays in the list as `status: "deleting"` until its teardown
 * lands, and an OLTP branch reports `cutting` → `migrating` → `ready` as the worker
 * re-cuts it, so the panel polls while the server is still working on any row
 * (`settling`): a row that vanished on click and a row still being torn down are
 * different answers, and so are a branch that serves and one that is about to.
 *
 * A 403 — the caller may not open the app's non-production environments — is an
 * answer, not a fault, so it is not retried: the Secrets pane's picker and this
 * panel each say so as soon as it arrives.
 */
export const useAppEnvironments = (appId: string | undefined) =>
  useQuery({
    queryKey: queryKeys.customApps.environments(appId ?? ""),
    queryFn: () => CustomAppsService.listEnvironments(appId as string),
    enabled: !!appId,
    retry: (failures, error) => !isForbidden(error) && failures < 3,
    staleTime: 15_000,
    refetchInterval: (query) => (query.state.data?.some(settling) ? 5_000 : false)
  });

export const useCreateAppEnvironment = (appId: string) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => CustomAppsService.createEnvironment(appId, name),
    onSuccess: (env) => {
      qc.invalidateQueries({ queryKey: queryKeys.customApps.environments(appId) });
      toast.success(`Sandbox ${env.name} created`);
    },
    onError: (err) => toast.error(errMessage(err, "Couldn't create the sandbox"))
  });
};

export const useDeleteAppEnvironment = (appId: string) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (name: string) => CustomAppsService.deleteEnvironment(appId, name),
    onSuccess: (accepted) => {
      qc.invalidateQueries({ queryKey: queryKeys.customApps.environments(appId) });
      // The sandbox's secrets go with it.
      qc.invalidateQueries({ queryKey: queryKeys.customApps.secrets(appId) });
      toast.success(`Tearing down ${accepted.name}`);
    },
    onError: (err) => toast.error(errMessage(err, "Couldn't delete the sandbox"))
  });
};

/**
 * What a `capture` environment kept instead of sending (`GET …/outbox`), newest
 * first. Only asked for while `enabled`, so a closed drawer reads nothing.
 */
export const useEnvironmentOutbox = (appId: string, name: string, enabled: boolean) =>
  useQuery({
    queryKey: queryKeys.customApps.environmentOutbox(appId, name),
    queryFn: () => CustomAppsService.environmentOutbox(appId, name),
    enabled: enabled && !!appId,
    retry: (failures, error) => !isForbidden(error) && failures < 2
  });

/**
 * Put planes back to a clean copy of their source (`POST …/reset`). Each plane
 * answers `done`, or `queued` for a re-cut that runs in the background (`oltp`),
 * so the list is refreshed either way.
 */
export const useResetEnvironmentPlanes = (appId: string) => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ name, planes }: { name: string; planes: string[] }) =>
      CustomAppsService.resetEnvironmentPlanes(appId, name, planes),
    onSuccess: (reset, { planes }) => {
      qc.invalidateQueries({ queryKey: queryKeys.customApps.environments(appId) });
      // The Secrets pane lists them under its own key.
      if (planes.includes("secrets")) {
        qc.invalidateQueries({ queryKey: queryKeys.customApps.secrets(appId) });
      }
      const queued = reset.planes.some((p) => p.status === "queued");
      toast.success(queued ? `Resetting ${reset.name} in the background` : `Reset ${reset.name}`);
    },
    onError: (err) => toast.error(errMessage(err, "Couldn't reset the plane"))
  });
};
