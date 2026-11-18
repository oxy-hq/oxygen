import { useMemo } from "react";
import { useLocation } from "react-router-dom";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * The breadcrumb label for the current core-app page. The TopBar is hidden
 * inside `/ide` and the setup wizard, `/setup` (the rail hides there too), so those never
 * need a label. Custom apps are full-page surfaces outside the SPA, so their
 * names belong to the SDK top bar (follow-up), not here.
 */
export function usePageName(): string {
  const { pathname } = useLocation();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { workspace } = useCurrentWorkspace();
  const ws = ROUTES.ORG(orgSlug).WORKSPACE(workspace?.id ?? "");

  return useMemo(() => {
    // Most specific first.
    if (pathname.startsWith(`${ws.THREADS}/`)) return "Thread";
    if (pathname === ws.THREADS) return "Chat";
    if (pathname.startsWith(ws.WORKFLOWS)) return "Automations";
    if (pathname.includes("/pipelines/")) return "Pipeline";
    if (pathname === ws.CONTEXT_GRAPH) return "Context Graph";
    if (pathname === ws.WORLD_MODEL) return "World model";
    // Home / index / anything else → the home.
    return "Home";
  }, [pathname, ws.THREADS, ws.WORKFLOWS, ws.CONTEXT_GRAPH, ws.WORLD_MODEL]);
}
