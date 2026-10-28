import type { ComponentType } from "react";
import { Navigate, useParams } from "react-router-dom";
import { SidebarProvider } from "@/components/ui/shadcn/sidebar";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useOrgs } from "@/hooks/api/organizations";
import { useAllWorkspaces } from "@/hooks/api/workspaces/useWorkspaces";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import { useLandingWorkspace } from "@/hooks/useLandingWorkspace";
import ROUTES from "@/libs/utils/routes";

/** The props the workspace layout takes to render the org home. */
export interface OrgHomeLayoutProps {
  workspaceId?: string;
  orgHome?: boolean;
}

/**
 * `/<org>/home`: the org's home. Its grid lists every app the person can open
 * across the org's workspaces; the rest of the shell (Chat, threads, the Ask
 * dock, setup readiness, the config banner, Settings) still loads from one
 * workspace. That workspace is chosen as the landing always was: the remembered
 * one, else the one with the most apps (`useLandingWorkspace`).
 *
 * Like `OrgDispatcher` it resolves the org from the URL slug, not the store:
 * `OrgGuard` primes the store in an effect that runs after this renders.
 *
 * With no navigable workspace there is nothing to load from, so this goes back
 * to the org root, which shows "being set up". The root only sends anyone here
 * once it has found a workspace, so the two never bounce.
 */
/** `Layout` is the workspace layout, passed in because it lives in `App.tsx`,
 *  which imports this file. */
export default function OrgHome({ Layout }: { Layout: ComponentType<OrgHomeLayoutProps> }) {
  const { orgSlug } = useParams<{ orgSlug: string }>();
  const { data: orgs, isPending: orgsPending } = useOrgs();
  const org = orgs?.find((o) => o.slug === orgSlug);
  const { data: workspaces, isPending: wsPending, isError } = useAllWorkspaces(org?.id);
  const landing = useLandingWorkspace(workspaces, org?.id);
  const appsFirst = useAppsFirstExperience();

  if (!orgSlug) return <Navigate to={ROUTES.ROOT} replace />;
  // The org home is the apps-first experience's. Without it, `/<org>/home` goes to
  // the org root, which lands in a workspace as before.
  if (!appsFirst) return <Navigate to={ROUTES.ORG(orgSlug).ROOT} replace />;
  if (orgsPending || !org || wsPending || landing.isPending) return <HomeSpinner />;
  if (isError || !landing.target) {
    return <Navigate to={ROUTES.ORG(orgSlug).ROOT} replace />;
  }

  return (
    <SidebarProvider>
      <Layout workspaceId={landing.target.id} orgHome />
    </SidebarProvider>
  );
}

function HomeSpinner() {
  return (
    <div className='flex h-full w-full items-center justify-center'>
      <Spinner className='size-6' />
    </div>
  );
}
