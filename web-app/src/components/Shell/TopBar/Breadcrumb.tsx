import { Link } from "react-router-dom";
import useHomeV3 from "@/hooks/useHomeV3";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import { usePageName } from "./usePageName";

/**
 * "<Org> / <Page>" — e.g. "Poke House / Home". The name is a link back to the
 * org home. Page identity lives here, replacing the launcher's
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
  const home = ROUTES.ORG(orgSlug).HOME;

  // Org name in cloud; workspace name as a fallback; plain label in local mode.
  const workspaceName = orgName || workspace?.name || "Workspace";
  // On home v3 the org's name is the whole crumb: "/ Home" says nothing.
  const homeV3 = useHomeV3();
  const atHome = homeV3 && pageName === "Home";

  return (
    <div className='flex min-w-0 items-center gap-2 text-sm'>
      <Link
        to={home}
        data-testid='topbar-workspace-link'
        className='min-w-0 truncate font-medium text-foreground transition-opacity hover:opacity-70'
      >
        {workspaceName}
      </Link>
      {!atHome && (
        <>
          <span className='shrink-0 text-muted-foreground/40'>/</span>
          <span className='shrink-0 text-muted-foreground'>{pageName}</span>
        </>
      )}
    </div>
  );
}
