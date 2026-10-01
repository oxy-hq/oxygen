import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { toast } from "sonner";
import { AdminPartnersService } from "@/services/api/adminPartners";
import type { AdminPartnerCapabilities } from "@/types/adminPartners";
import queryKeys from "../queryKey";

export const useAdminPartners = () =>
  useQuery({
    queryKey: queryKeys.adminPartner.list(),
    queryFn: () => AdminPartnersService.list()
  });

/** `orgId` — a partner IS an org, so its detail is keyed on the org id. */
export const useAdminPartnerDetail = (orgId: string | undefined) =>
  useQuery({
    queryKey: queryKeys.adminPartner.detail(orgId ?? ""),
    queryFn: () => AdminPartnersService.get(orgId as string),
    enabled: !!orgId
  });

/** Invalidate the list + a partner's detail after a mutation. */
function useInvalidate() {
  const qc = useQueryClient();
  return (orgId?: string) => {
    qc.invalidateQueries({ queryKey: queryKeys.adminPartner.list() });
    if (orgId) {
      qc.invalidateQueries({ queryKey: queryKeys.adminPartner.detail(orgId) });
    }
  };
}

/** Withdraw the partnership. The org survives — only its reach over others goes. */
export const useRevokePartnership = () => {
  const invalidate = useInvalidate();
  return useMutation({
    mutationFn: (orgId: string) => AdminPartnersService.revoke(orgId),
    onSuccess: () => {
      invalidate();
      toast.success("Partnership revoked");
    },
    onError: () => toast.error("Failed to revoke the partnership")
  });
};

/**
 * Raise or lower the CEILING — the maximum this partner can ever do. A 403 here
 * means the actor is a Global Admin trying to grant billing/secrets, which stays
 * Owner-only.
 */
export const useSetPartnerCapabilities = (orgId: string) => {
  const invalidate = useInvalidate();
  return useMutation({
    mutationFn: (caps: AdminPartnerCapabilities) =>
      AdminPartnersService.setCapabilities(orgId, caps),
    onSuccess: () => {
      invalidate(orgId);
      toast.success("Ceiling updated");
    },
    onError: (e: unknown) =>
      toast.error(
        isForbidden(e)
          ? "Only a Global Owner can grant billing or secrets access"
          : "Failed to update the ceiling"
      )
  });
};

export const useAttachPartnerOrg = (orgId: string) => {
  const invalidate = useInvalidate();
  return useMutation({
    mutationFn: (managedOrgId: string) => AdminPartnersService.attachOrg(orgId, managedOrgId),
    onSuccess: () => {
      invalidate(orgId);
      toast.success("Client attached");
    },
    onError: (e: unknown) =>
      toast.error(
        isConflict(e)
          ? "That org is already managed by another partner"
          : "Failed to attach the client"
      )
  });
};

export const useDetachPartnerOrg = (orgId: string) => {
  const invalidate = useInvalidate();
  return useMutation({
    mutationFn: (managedOrgId: string) => AdminPartnersService.detachOrg(orgId, managedOrgId),
    onSuccess: () => invalidate(orgId),
    onError: () => toast.error("Failed to detach the client")
  });
};

/** Staff grant/revoke of partner access for a member of the partner org. */
export const useSetPartnerPersonAccess = (orgId: string) => {
  const invalidate = useInvalidate();
  return useMutation({
    mutationFn: ({ orgMemberId, hasAccess }: { orgMemberId: string; hasAccess: boolean }) =>
      AdminPartnersService.setPersonAccess(orgId, orgMemberId, hasAccess),
    onSuccess: (_data, { hasAccess }) => {
      invalidate(orgId);
      toast.success(hasAccess ? "Partner access granted" : "Partner access revoked");
    },
    onError: () => toast.error("Failed to change partner access")
  });
};

function status(e: unknown): number | undefined {
  return typeof e === "object" && e !== null && "response" in e
    ? (e as { response?: { status?: number } }).response?.status
    : undefined;
}

function isConflict(e: unknown): boolean {
  return status(e) === 409;
}

function isForbidden(e: unknown): boolean {
  return status(e) === 403;
}
