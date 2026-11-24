import { useEffect } from "react";
import { useParams } from "react-router-dom";
import { useResolveOrgForGuard } from "@/components/useResolveOrgForGuard";
import { useAllWorkspaces, useWorkspace } from "@/hooks/api/workspaces/useWorkspaces";
import { useLandingWorkspace } from "@/hooks/useLandingWorkspace";
import useOauthConnectReturn from "@/hooks/useOauthConnectReturn";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import SettingsPage from "./SettingsPage";

/**
 * Loads the org's workspace for the settings route, and renders settings either way.
 *
 * Settings sits at the org level rather than inside the workspace layout, because an org
 * still being set up has no workspace and its "being set up" screen links into Crew,
 * Locations and App access — none of which need one. But the workspace layout is also what
 * *fetches* the workspace, so without this the Data group and My activity were missing for
 * everyone: `useVisibleNavGroups` reads `hasWorkspace`, and nothing had set it.
 *
 * `OrgWorkspace` cannot be reused here: it redirects to the org root when there is no
 * workspace, which is exactly the case this route has to keep serving.
 */
export default function SettingsWorkspaceGate() {
  const { orgSlug } = useParams<{ orgSlug: string }>();
  const { org, isPending: orgPending } = useResolveOrgForGuard(orgSlug);
  const { data: workspaces, isPending: workspacesPending } = useAllWorkspaces(org?.id);
  const landing = useLandingWorkspace(workspaces, org?.id);
  const workspaceId = landing.target?.id;

  const { data } = useWorkspace(workspaceId ?? "");
  const setWorkspace = useCurrentWorkspace((s) => s.setWorkspace);
  const stored = useCurrentWorkspace((s) => s.workspace);

  useEffect(() => {
    if (data) setWorkspace(data);
  }, [data, setWorkspace]);

  // Connections starts OAuth with `return_path: window.location.href`, which is now a
  // settings address — and settings renders outside `WorkspaceLayout`, the only place this
  // hook used to run. So the provider's return rendered this component, the hook never
  // fired, and there was no "Connected" toast and no cleanup of `?oxy_connected`: by the
  // hook's own doc, indistinguishable from having cancelled.
  useOauthConnectReturn();

  // Deliberately not gated on the fetch: an org with no workspace must still reach its
  // org-level pages, and one that has a workspace shows those sections as soon as it lands —
  // the nav simply grows, rather than the whole surface waiting.
  //
  // `contextReady` says when the gates can be trusted, which is a different question: until
  // the org, the role and the workspace have all answered, "the nav does not have this
  // section" means "not yet", not "not for you".
  //
  // Read from the **store**, not from the query. Keyed on `data` this flipped true on the
  // render the fetch resolved, one commit before the effect above writes the store — and in
  // that render `useVisibleNavGroups` still sees no workspace, so the Data sections are not
  // in the nav. Child effects run before parent ones, so the `<Navigate replace>` in
  // `SettingsPage` fired before `setWorkspace`, and every cold load of a workspace-gated
  // address was bounced: refresh, a bookmark, a shared link, the OAuth return to
  // Connections. `landing.isPending` is in here too — `workspaceId` is undefined while the
  // app counts that pick the workspace are still arriving, which would read as "no
  // workspace at all".
  const contextReady =
    !orgPending &&
    !workspacesPending &&
    !landing.isPending &&
    (!workspaceId || stored?.id === workspaceId);
  return <SettingsPage contextReady={contextReady} />;
}
