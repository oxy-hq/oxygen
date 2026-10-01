import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { isAxiosError } from "axios";
import { toast } from "sonner";
import {
  type AdminCreateOrgBody,
  AdminOrgsService,
  type ListOrgsMetaQuery,
  type RenameOrgBody
} from "@/services/api/adminTenants";
import queryKeys from "../queryKey";

/** Server hard-caps `page_size` at 200; request at the cap so a caller draining
 *  every page issues the fewest requests. */
const ORGS_PAGE_SIZE = 200;

function errMessage(err: unknown, fallback: string): string {
  if (isAxiosError(err)) return err.response?.data?.message ?? err.message;
  if (err instanceof Error) return err.message;
  return fallback;
}

/** Create-org failures are status-only (no message body), so map the two the
 *  handler returns to something a human can act on. */
function createOrgError(err: unknown): string {
  if (isAxiosError(err)) {
    if (err.response?.status === 409) return "That slug is already taken.";
    if (err.response?.status === 422)
      return "Check the details — the slug may be reserved or the owner email invalid.";
  }
  return errMessage(err, "Failed to create organization");
}

export const useCreateAdminOrg = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (body: AdminCreateOrgBody) => AdminOrgsService.create(body),
    onSuccess: (data) => {
      qc.invalidateQueries({ queryKey: queryKeys.adminOrgs.all });
      const { org, owner_email, owner_status } = data;
      toast.success(
        owner_status === "seeded"
          ? `Created ${org.name}. ${owner_email} is now the owner.`
          : `Created ${org.name}. Invited ${owner_email} to claim ownership.`
      );
    },
    onError: (err) => toast.error(createOrgError(err))
  });
};

export const useAdminOrgsList = (
  query: ListOrgsMetaQuery = {},
  options: { enabled?: boolean } = {}
) =>
  useQuery({
    queryKey: queryKeys.adminOrgs.list(query.search),
    queryFn: () => AdminOrgsService.list(query),
    enabled: options.enabled ?? true
  });

/**
 * Every org on the deployment, across all pages. Callers drain it by
 * auto-calling `fetchNextPage` while `hasNextPage` — the pattern the admin Apps
 * list uses — so counts and the "no access" set are exhaustive, not capped at
 * one page.
 *
 * "Is there more" is inferred from a FULL PAGE, because the response body is a
 * bare array with no total in it. That is correct but costs one extra empty
 * request when the row count is an exact multiple of the page size. The
 * endpoint now also answers `Link: <…>; rel="next"` (`oxy_app_core::pagination`,
 * exposed through CORS), which is exact — reading it here means threading
 * response headers through `AdminOrgsService.list`, so it is left as the
 * obvious next cleanup rather than done half-way.
 */
export const useAllAdminOrgs = (enabled = true) =>
  useInfiniteQuery({
    enabled,
    queryKey: [...queryKeys.adminOrgs.all, "all-pages"] as const,
    queryFn: ({ pageParam }) =>
      AdminOrgsService.list({ page: pageParam, page_size: ORGS_PAGE_SIZE }),
    initialPageParam: 0,
    getNextPageParam: (lastPage, _all, lastPageParam) =>
      lastPage.length === ORGS_PAGE_SIZE ? lastPageParam + 1 : undefined
  });

export const useAdminOrgDetail = (orgId: string | undefined) =>
  useQuery({
    queryKey: queryKeys.adminOrgs.detail(orgId ?? ""),
    queryFn: () => AdminOrgsService.detail(orgId as string),
    enabled: !!orgId
  });

export const useRenameAdminOrg = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ orgId, body }: { orgId: string; body: RenameOrgBody }) =>
      AdminOrgsService.rename(orgId, body),
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: queryKeys.adminOrgs.all });
      qc.invalidateQueries({ queryKey: queryKeys.adminOrgs.detail(vars.orgId) });
      toast.success("Organization updated");
    },
    onError: (err) => toast.error(errMessage(err, "Failed to update organization"))
  });
};

export const useDeleteAdminOrg = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (orgId: string) => AdminOrgsService.remove(orgId),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: queryKeys.adminOrgs.all });
      toast.success("Organization deleted");
    },
    onError: (err) => toast.error(errMessage(err, "Failed to delete organization"))
  });
};

/** Read a Blob as a `data:` URL — cache-friendly (a string, unlike an object
 *  URL that needs revoking) and small enough for the ≤1 MB logo cap. */
function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });
}

/** The org's uploaded logo as a `data:` URL, or `null` if none. */
export const useAdminOrgLogo = (orgId: string | undefined) =>
  useQuery({
    queryKey: queryKeys.adminOrgs.logo(orgId ?? ""),
    queryFn: async () => {
      const blob = await AdminOrgsService.getLogo(orgId as string);
      return blob ? await blobToDataUrl(blob) : null;
    },
    enabled: !!orgId,
    staleTime: 5 * 60_000
  });

export const useUploadAdminOrgLogo = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ orgId, file }: { orgId: string; file: File }) =>
      AdminOrgsService.uploadLogo(orgId, file),
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: queryKeys.adminOrgs.logo(vars.orgId) });
      qc.invalidateQueries({ queryKey: queryKeys.adminOrgs.detail(vars.orgId) });
      qc.invalidateQueries({ queryKey: queryKeys.adminOrgs.all });
      toast.success("Logo updated");
    },
    onError: (err) => toast.error(errMessage(err, "Failed to update logo"))
  });
};

export const useDeleteAdminOrgLogo = () => {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (orgId: string) => AdminOrgsService.deleteLogo(orgId),
    onSuccess: (_data, orgId) => {
      qc.invalidateQueries({ queryKey: queryKeys.adminOrgs.logo(orgId) });
      qc.invalidateQueries({ queryKey: queryKeys.adminOrgs.detail(orgId) });
      qc.invalidateQueries({ queryKey: queryKeys.adminOrgs.all });
      toast.success("Logo removed");
    },
    onError: (err) => toast.error(errMessage(err, "Failed to remove logo"))
  });
};
