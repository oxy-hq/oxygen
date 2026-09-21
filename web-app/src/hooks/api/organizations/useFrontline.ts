import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { awaitingTablet } from "@/libs/frontline";
import { FrontlineService } from "@/services/api/frontline";
import type {
  CreateKioskDeviceRequest,
  EnrolWorkerRequest,
  UpdateKioskDeviceRequest
} from "@/types/frontline";
import queryKeys from "../queryKey";

/**
 * Crew admin: the org's frontline workers and the kiosks they sign in on.
 *
 * Every route is org-admin only, so callers pass `enabled` from their own
 * `canManage` — firing the list for a plain member would trip the global 403
 * toast for a panel the nav never shows them.
 */
export const useFrontlineWorkers = (orgId: string, enabled = true) =>
  useQuery({
    queryKey: queryKeys.org.frontlineWorkers(orgId),
    queryFn: async () => (await FrontlineService.listWorkers(orgId)).workers,
    enabled: enabled && !!orgId
  });

export const useFrontlineDevices = (orgId: string, enabled = true) =>
  useQuery({
    queryKey: queryKeys.org.frontlineDevices(orgId),
    queryFn: async () => (await FrontlineService.listDevices(orgId)).devices,
    enabled: enabled && !!orgId,
    // An admin creates a kiosk, hands the link to the tablet and watches the
    // row. The bind lands on that other device, so poll — but only while a
    // link is live and unspent; a settled list costs nothing.
    refetchInterval: (query) =>
      query.state.data && awaitingTablet(query.state.data) ? 5_000 : false
  });

/**
 * Every worker mutation invalidates the worker list: standing, grants and the
 * lockout a PIN reset clears are all columns of that one table.
 */
const useWorkerMutation = <TVars extends { orgId: string }, TData>(
  mutationFn: (vars: TVars) => Promise<TData>
) => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: (_data, vars) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.org.frontlineWorkers(vars.orgId) });
    }
  });
};

export const useEnrolWorker = () =>
  useWorkerMutation((vars: { orgId: string; request: EnrolWorkerRequest }) =>
    FrontlineService.enrolWorker(vars.orgId, vars.request)
  );

export const useSetWorkerStanding = () =>
  useWorkerMutation((vars: { orgId: string; userId: string; active: boolean }) =>
    FrontlineService.setWorkerStanding(vars.orgId, vars.userId, vars.active)
  );

export const useSetWorkerApps = () =>
  useWorkerMutation((vars: { orgId: string; userId: string; apps: string[] }) =>
    FrontlineService.setWorkerApps(vars.orgId, vars.userId, vars.apps)
  );

export const useResetWorkerPin = () =>
  useWorkerMutation((vars: { orgId: string; userId: string; pin: string }) =>
    FrontlineService.resetWorkerPin(vars.orgId, vars.userId, vars.pin)
  );

const useDeviceMutation = <TVars extends { orgId: string }, TData>(
  mutationFn: (vars: TVars) => Promise<TData>
) => {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn,
    onSuccess: (_data, vars) => {
      queryClient.invalidateQueries({ queryKey: queryKeys.org.frontlineDevices(vars.orgId) });
    }
  });
};

/**
 * The result carries the enrol link exactly once. The caller must hand it to
 * the user immediately — it is not cached and cannot be fetched again.
 */
export const useCreateDevice = () =>
  useDeviceMutation((vars: { orgId: string; request: CreateKioskDeviceRequest }) =>
    FrontlineService.createDevice(vars.orgId, vars.request)
  );

/** Like `useCreateDevice`, the link in the result is shown once and not cached. */
export const useReissueEnrolLink = () =>
  useDeviceMutation((vars: { orgId: string; deviceId: string }) =>
    FrontlineService.reissueEnrolLink(vars.orgId, vars.deviceId)
  );

/**
 * Change a kiosk that is already on a counter. Invalidates the device list like
 * its siblings, so the row an admin just edited re-reads from the server rather
 * than from the response alone — the list is polled while any enrol link is
 * live, and two sources for one row would race.
 */
export const useUpdateDevice = () =>
  useDeviceMutation(
    (vars: { orgId: string; deviceId: string; request: UpdateKioskDeviceRequest }) =>
      FrontlineService.updateDevice(vars.orgId, vars.deviceId, vars.request)
  );

export const useRevokeDevice = () =>
  useDeviceMutation((vars: { orgId: string; deviceId: string }) =>
    FrontlineService.revokeDevice(vars.orgId, vars.deviceId)
  );
