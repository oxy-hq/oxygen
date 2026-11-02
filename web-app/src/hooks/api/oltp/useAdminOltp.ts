import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import queryKeys from "@/hooks/api/queryKey";
import {
  AdminOltpService,
  listOltpTenants,
  type OltpConnectionTest,
  type OltpCredentials
} from "@/services/api/oltp";
import { errMessage } from "../errMessage";

// `web-app/CLAUDE.md` is a hard rule: every key comes from `queryKey.ts`. The
// inline arrays these replaced were spelled out in three places, so a rename
// would have left `invalidateQueries` pointed at a key nothing reads — a cache
// that silently stops refreshing rather than failing.
const key = queryKeys.oltp.admin;
const tenantsKey = queryKeys.oltp.adminTenants;

/** OLTP status for one org. `enabled` so the tab only queries when opened. */
export function useAdminOltpStatus(orgId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: key(orgId ?? ""),
    queryFn: () => AdminOltpService.getStatus(orgId as string),
    enabled: Boolean(orgId) && enabled,
    // Provisioning is slow and rare; nothing else mutates this behind our back.
    staleTime: 30_000
  });
}

export function useProvisionOltp(orgId: string | undefined) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (writers: string[]) => AdminOltpService.provision(orgId as string, writers),
    onSuccess: (data) => {
      // Seed the cache from the response rather than refetching: provision
      // already returns the post-provision status, and a refetch here would
      // race a scale-to-zero database waking up.
      qc.setQueryData(key(orgId ?? ""), data);
      // Same reason as deprovision: the fleet list's counts just changed.
      void qc.invalidateQueries({ queryKey: tenantsKey() });
      toast.success(`Provisioned ${data.database}`);
    },
    onError: (err) => toast.error(errMessage(err, "Could not provision OLTP"))
  });
}

/**
 * Probe the org's database on demand.
 *
 * A mutation, like `useOltpCredentials`: it is never cached or refetched,
 * because each call opens a real session and wakes a suspended compute. A
 * refusal is data (`ok: false`), not an error — `onError` is for the probe
 * itself failing.
 */
export function useTestOltpConnection(orgId: string | undefined) {
  return useMutation<OltpConnectionTest, unknown, void>({
    mutationFn: () => AdminOltpService.testConnection(orgId as string),
    onError: (err) => toast.error(errMessage(err, "Could not test the connection"))
  });
}

/**
 * Fetch a DSN on demand.
 *
 * A mutation rather than a query on purpose: this is not cached, not
 * refetched, and not prefetched — each disclosure is a deliberate act the
 * server records.
 */
export function useOltpCredentials(orgId: string | undefined) {
  return useMutation<OltpCredentials, unknown, string>({
    mutationFn: (role: string) => AdminOltpService.credentials(orgId as string, role),
    onError: (err) => toast.error(errMessage(err, "Could not fetch credentials"))
  });
}

export function useSetOltpVisibility(orgId: string | undefined) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ writer, visible }: { writer: string; visible: boolean }) =>
      AdminOltpService.setVisibility(orgId as string, writer, visible),
    onSuccess: (data, vars) => {
      qc.setQueryData(key(orgId ?? ""), data);
      // The fleet list renders visibility as chip fill now, so leaving it
      // un-invalidated showed the pre-toggle grant for up to its 30s
      // staleTime — the one number on that page this mutation changes.
      // provision and deprovision already did this.
      void qc.invalidateQueries({ queryKey: tenantsKey() });
      toast.success(vars.visible ? "Analytics can read it" : "Hidden from analytics");
    },
    onError: (err) => toast.error(errMessage(err, "Could not change visibility"))
  });
}

export function useDeprovisionOltp(orgId: string | undefined) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => AdminOltpService.deprovision(orgId as string),
    onSuccess: (data) => {
      qc.setQueryData(key(orgId ?? ""), data);
      // The fleet list counts provisioned orgs, so it is now stale.
      void qc.invalidateQueries({ queryKey: tenantsKey() });
      toast.success("Deprovisioned");
    },
    onError: (err) => toast.error(errMessage(err, "Could not deprovision"))
  });
}

/** The fleet-wide list backing the OLTP sidebar page. */
export function useOltpTenants() {
  return useQuery({
    queryKey: tenantsKey(),
    queryFn: listOltpTenants,
    staleTime: 30_000
  });
}
