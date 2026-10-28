import type { Workspace } from "@/types/workspace";

/**
 * Whether to offer the Oxygen Factory (the IDE and back office) to the caller in
 * this workspace.
 *
 * The server decides — Oxy staff, an owner or admin of the org, or a member an
 * operator granted the workspace to
 * (`GET /{workspace}/meta` → `factory_access`) — and sends `true` to everyone while
 * enforcement is off. Only an explicit `false` hides it: a response that predates
 * the field, or a workspace not loaded yet, shows what the rail always showed.
 *
 * This is presentation, not access control. Every Factory route refuses the rest
 * with `403 factory_access_required` whatever a client does with this.
 */
export function canUseFactory(workspace: Pick<Workspace, "factory_access"> | null | undefined) {
  return workspace?.factory_access !== false;
}

/** The `code` the server answers a Factory route with when the caller may not use it. */
export const FACTORY_ACCESS_REQUIRED = "factory_access_required";
