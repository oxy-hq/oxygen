import type { SettingsSection } from "@/components/settings/settingsRoute";
import { useAuth } from "@/contexts/AuthContext";
import useCanUsePreviews from "@/hooks/useCanUsePreviews";
import { useRole } from "@/hooks/useRole";
import { canUseFactory } from "@/libs/utils/factoryAccess";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import { type VisibleNavGroup, visibleNavGroups } from "./nav";

/**
 * The settings nav the signed-in viewer sees, from the live role, org and workspace. The dialog
 * renders it; a section that links to another (API tokens' guide pointing at Secrets) asks it
 * whether the viewer could follow that link, so the two never disagree.
 */
export function useVisibleNavGroups(): VisibleNavGroup[] {
  const { isLocalMode, authConfig } = useAuth();
  const { org } = useCurrentOrg();
  const role = useCurrentOrg((s) => s.role);
  const { workspace } = useCurrentWorkspace();
  const { is } = useRole();
  const isStaff = !!useCanUsePreviews();

  return visibleNavGroups({
    isLocalMode,
    isOrgAdmin: is.orgAdmin,
    isWorkspaceAdmin: is.workspaceAdmin,
    isStaff,
    canUseFactory: canUseFactory(workspace),
    billingEnabled: authConfig.billing_enabled,
    hasOrg: !!org && !!role,
    hasWorkspace: !!workspace,
    orgName: org?.name
  });
}

/** Whether the viewer's nav holds `section`. */
export function useCanOpenSection(section: SettingsSection): boolean {
  return useVisibleNavGroups().some((group) => group.items.some((item) => item.value === section));
}
