import ROUTES from "@/libs/utils/routes";
import type { OxyAccessRow } from "@/types/apps";

/**
 * Open a granted workspace's main UI (/home) in a NEW TAB from the admin
 * browser, so the operator stays on the access list.
 *
 * Goes through the entry link (`ENTER_WORKSPACE(id).HOME`): it remembers this
 * workspace as the operator's for that org in this browser, then lands on
 * `/<org>/home`, which loads from it. A workspace that is not navigable yet
 * (still cloning) is not honoured, so the tab opens on the org's landing pick
 * instead. Requires the backend workspace middleware to grant Global
 * Owners/Admins access to non-member orgs' workspaces (mirrors `org_middleware`).
 */
export function openWorkspaceHome(grant: OxyAccessRow): void {
  window.open(
    ROUTES.ORG(grant.org_slug).ENTER_WORKSPACE(grant.workspace_id).HOME,
    "_blank",
    "noopener"
  );
}
