import { Navigate, useLocation, useParams } from "react-router-dom";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useResolveOrgForGuard } from "@/components/useResolveOrgForGuard";
import { setLastWorkspaceId } from "@/libs/utils/lastWorkspace";
import ROUTES from "@/libs/utils/routes";
import { entryRedirectPath } from "@/libs/utils/workspaceEntry";

/**
 * `/<org>/workspaces/<id>/<page>`: the old address of a workspace page, and the
 * link that opens one particular workspace. It remembers the workspace as this
 * person's for the org, then replaces itself with `/<org>/<page>`, which loads
 * from the remembered workspace (`OrgWorkspace`). The query string, the hash and
 * the navigation state ride along: a notification's `?run=`, a preview pin's
 * `?preview=`, `?settings=`.
 *
 * An id that is not one of the org's workspaces, or no longer navigable, is
 * remembered harmlessly: the landing rule only honours a remembered workspace
 * that is in the org's list, so the page opens on the org's own.
 */
export default function WorkspaceEntry() {
  const { orgSlug, wsId } = useParams<{ orgSlug: string; wsId: string }>();
  const location = useLocation();
  const { org, isPending } = useResolveOrgForGuard(orgSlug);

  if (!orgSlug) return <Navigate to={ROUTES.ROOT} replace />;
  if (isPending) {
    return (
      <div className='flex h-full w-full items-center justify-center'>
        <Spinner className='size-6' />
      </div>
    );
  }
  // Written before the redirect renders the next route, which reads it.
  if (org && wsId) setLastWorkspaceId(org.id, wsId);

  const pathname = entryRedirectPath(location.pathname) ?? ROUTES.ORG(orgSlug).HOME;
  return (
    <Navigate
      to={{ pathname, search: location.search, hash: location.hash }}
      state={location.state}
      replace
    />
  );
}
