import { Link } from "react-router-dom";
import useAppsFirstExperience from "@/hooks/useAppsFirstExperience";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import { usePageName } from "./usePageName";

/**
 * "<Workspace> / <Page>" — e.g. "Poke House / HQ". The workspace name is a
 * link back to the HQ home. Page identity lives here, replacing the launcher's
 * removed HQ heading + status line. The workspace logo is intentionally NOT
 * shown — the rail's workspace tile already carries it, so a second copy here
 * is redundant. Where space runs out (a phone, a long org name) the name
 * truncates rather than run under the bell; the page name is always one short word.
 */
export function Breadcrumb() {
  const orgName = useCurrentOrg((s) => s.org?.name);
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { workspace } = useCurrentWorkspace();
  const pageName = usePageName();
  // The org home is the apps-first experience's; without it, the workspace's.
  const appsFirst = useAppsFirstExperience();
  const home = appsFirst
    ? ROUTES.ORG(orgSlug).HOME
    : ROUTES.ORG(orgSlug).WORKSPACE(workspace?.id ?? "").HOME;

  // Org name in cloud; workspace name as a fallback; plain label in local mode.
  const workspaceName = orgName || workspace?.name || "Workspace";

  return (
    <div className='flex min-w-0 items-center gap-2 text-sm'>
      <Link
        to={home}
        data-testid='topbar-workspace-link'
        className='min-w-0 truncate font-medium text-foreground transition-opacity hover:opacity-70'
      >
        {workspaceName}
      </Link>
      <span className='shrink-0 text-muted-foreground/40'>/</span>
      <span className='shrink-0 text-muted-foreground'>{pageName}</span>
    </div>
  );
}
