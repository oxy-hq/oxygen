import { type ComponentType, useState } from "react";
import { Navigate, useParams } from "react-router-dom";
import { SidebarProvider } from "@/components/ui/shadcn/sidebar";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useResolveOrgForGuard } from "@/components/useResolveOrgForGuard";
import { useAllWorkspaces } from "@/hooks/api/workspaces/useWorkspaces";
import { useLandingWorkspace } from "@/hooks/useLandingWorkspace";
import ROUTES from "@/libs/utils/routes";

/** The props the workspace layout takes to render the org's pages. */
export interface OrgWorkspaceLayoutProps {
  workspaceId?: string;
}

/** The workspace this mount of the shell loads from, and the org it was picked in. */
type Pin = { orgId: string; workspaceId: string };

/**
 * `/<org>/<page>`: every page of the org — home, chat, automations, the Factory —
 * rendered by the workspace layout from the org's workspace. An org has one, so
 * the address names only the org. Where an org still has several, until they
 * are migrated, it is the landing rule's pick (`useLandingWorkspace`): the one
 * this person opened last, which a `/<org>/workspaces/<id>/…` link sets
 * (`WorkspaceEntry`), else the one with the most apps.
 *
 * The pick is pinned while this route stays mounted, which covers every move
 * between the org's pages. The remembered workspace is per browser, so another
 * tab opening a different one must not move this tab's shell under it. A
 * different org picks again.
 *
 * Resolves the org as `OrgGuard` does, not from the store: the guard primes the
 * store in an effect that runs after this renders, and staff opening a tenant
 * they are not a member of resolve only through the guard's fallback.
 *
 * With no navigable workspace there is nothing to load from, so this goes back
 * to the org root, which shows "being set up". The root only sends anyone here
 * once it has found a workspace, so the two never bounce.
 *
 * `Layout` is the workspace layout, passed in because it lives in `App.tsx`,
 * which imports this file.
 */
export default function OrgWorkspace({
  Layout
}: {
  Layout: ComponentType<OrgWorkspaceLayoutProps>;
}) {
  const { orgSlug } = useParams<{ orgSlug: string }>();
  const { org, isPending: orgPending } = useResolveOrgForGuard(orgSlug);
  const { data: workspaces, isPending: wsPending, isError } = useAllWorkspaces(org?.id);
  const landing = useLandingWorkspace(workspaces, org?.id);
  const [pin, setPin] = useState<Pin | null>(null);

  const pinned = org && pin?.orgId === org.id ? pin.workspaceId : undefined;
  const target = landing.target;
  // Set while rendering, React's pattern for state derived from a change, so
  // this very render already uses the pick.
  if (org && target && !pinned) setPin({ orgId: org.id, workspaceId: target.id });
  const workspaceId = pinned ?? target?.id;

  if (!orgSlug) return <Navigate to={ROUTES.ROOT} replace />;
  if (workspaceId) {
    return (
      <SidebarProvider>
        <Layout workspaceId={workspaceId} />
      </SidebarProvider>
    );
  }
  if (orgPending || !org || wsPending || landing.isPending) return <PageSpinner />;
  if (isError || !target) return <Navigate to={ROUTES.ORG(orgSlug).ROOT} replace />;
  return <PageSpinner />;
}

function PageSpinner() {
  return (
    <div className='flex h-full w-full items-center justify-center'>
      <Spinner className='size-6' />
    </div>
  );
}
