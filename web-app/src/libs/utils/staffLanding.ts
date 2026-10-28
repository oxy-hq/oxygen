import type { UserInfo } from "@/types/auth";

/**
 * Whether a signed-in person is Oxy staff for landing purposes: an operator
 * (global owner or app admin) or a partner's staff. They work inside a workspace
 * — the Factory, a client's setup — so they land in the workspace they last
 * opened. Everyone else lands on the org home.
 */
export function landsInWorkspace(user: UserInfo | undefined): boolean {
  return !!(user?.is_owner || user?.is_app_admin || user?.partner_memberships?.length);
}
