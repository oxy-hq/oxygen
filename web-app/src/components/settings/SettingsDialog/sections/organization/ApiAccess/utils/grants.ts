import type { Grant, GrantInput, RoleCeiling } from "@/types/apiToken";
import type { ServiceAccountRole } from "@/types/orgApiAccess";

/** One picked workspace and the most its holder may be there. */
export interface WorkspaceGrantDraft {
  workspace_id: string;
  role_ceiling: RoleCeiling;
}

/**
 * What the access picker edits. `scope: "org"` is every workspace in the org,
 * including ones created later; `"selected"` is the listed workspaces only.
 * `appIds` are apps the holder may publish, independent of either.
 */
export interface AccessDraft {
  scope: "org" | "selected";
  workspaces: WorkspaceGrantDraft[];
  appIds: string[];
}

export const ORG_WIDE_ACCESS: AccessDraft = { scope: "org", workspaces: [], appIds: [] };
export const NOTHING_SELECTED: AccessDraft = { scope: "selected", workspaces: [], appIds: [] };

export const ROLE_LABELS: Record<RoleCeiling, string> = {
  viewer: "Viewer",
  member: "Member",
  admin: "Admin",
  owner: "Owner"
};

const RANK: Record<RoleCeiling, number> = { viewer: 0, member: 1, admin: 2, owner: 3 };

/**
 * The ceilings worth offering for an account. A ceiling above the account's
 * own role would promise reach it doesn't have, and a service account is
 * never an owner.
 */
export function ceilingOptions(accountRole: ServiceAccountRole): RoleCeiling[] {
  return accountRole === "admin" ? ["viewer", "member", "admin"] : ["viewer", "member"];
}

/** `ceiling`, lowered to the account's role when it is above it. */
export function clampCeiling(ceiling: RoleCeiling, accountRole: ServiceAccountRole): RoleCeiling {
  return RANK[ceiling] > RANK[accountRole] ? accountRole : ceiling;
}

export function toggleWorkspace(
  draft: AccessDraft,
  workspaceId: string,
  ceiling: RoleCeiling
): AccessDraft {
  const picked = draft.workspaces.some((w) => w.workspace_id === workspaceId);
  return {
    ...draft,
    workspaces: picked
      ? draft.workspaces.filter((w) => w.workspace_id !== workspaceId)
      : [...draft.workspaces, { workspace_id: workspaceId, role_ceiling: ceiling }]
  };
}

export function setWorkspaceCeiling(
  draft: AccessDraft,
  workspaceId: string,
  ceiling: RoleCeiling
): AccessDraft {
  return {
    ...draft,
    workspaces: draft.workspaces.map((w) =>
      w.workspace_id === workspaceId ? { ...w, role_ceiling: ceiling } : w
    )
  };
}

export function toggleApp(draft: AccessDraft, appId: string): AccessDraft {
  return {
    ...draft,
    appIds: draft.appIds.includes(appId)
      ? draft.appIds.filter((id) => id !== appId)
      : [...draft.appIds, appId]
  };
}

/** Why the draft grants nothing, or `null` when it grants something. */
export function accessDraftError(draft: AccessDraft, allowApps = false): string | null {
  if (draft.scope === "org") return null;
  if (draft.workspaces.length > 0) return null;
  if (allowApps && draft.appIds.length > 0) return null;
  return allowApps ? "Pick at least one workspace or one app." : "Pick at least one workspace.";
}

interface BuildOptions {
  /**
   * Send the org-wide grant explicitly instead of leaving `grants` out. A
   * service-account token omits it — the server then grants the whole org at
   * the account's role — but a trusted-access policy has no such default, so
   * it always says what it means.
   */
  explicitOrgWide?: boolean;
}

/**
 * The `grants` array for a create or update body, or `undefined` when the
 * body should carry none and take the server's default.
 */
export function buildGrantInputs(
  draft: AccessDraft,
  accountRole: ServiceAccountRole,
  { explicitOrgWide = false }: BuildOptions = {}
): GrantInput[] | undefined {
  const apps: GrantInput[] = draft.appIds.map((app_id) => ({ kind: "app_publish", app_id }));

  if (draft.scope === "org") {
    if (!explicitOrgWide && apps.length === 0) return undefined;
    return [{ kind: "workspace", workspace_id: null, role_ceiling: accountRole }, ...apps];
  }

  const workspaces: GrantInput[] = draft.workspaces.map((w) => ({
    kind: "workspace",
    workspace_id: w.workspace_id,
    role_ceiling: clampCeiling(w.role_ceiling, accountRole)
  }));
  return [...workspaces, ...apps];
}

const live = (grants: Grant[]) => grants.filter((g) => !g.revoked_at);

/** The picker state that reproduces a saved set of grants, for editing it. */
export function draftFromGrants(grants: Grant[]): AccessDraft {
  const active = live(grants);
  const workspaceGrants = active.filter((g) => g.kind === "workspace");
  const appIds = active.flatMap((g) => (g.kind === "app_publish" && g.app_id ? [g.app_id] : []));

  if (workspaceGrants.some((g) => g.workspace_id === null)) {
    return { scope: "org", workspaces: [], appIds };
  }
  return {
    scope: "selected",
    workspaces: workspaceGrants.flatMap((g) =>
      g.workspace_id
        ? [{ workspace_id: g.workspace_id, role_ceiling: g.role_ceiling ?? "member" }]
        : []
    ),
    appIds
  };
}

export interface AccessDescription {
  /** One short phrase for a table cell. */
  summary: string;
  /** One line per grant, for the detail behind the phrase. */
  details: string[];
}

const roleOf = (g: Grant) => ROLE_LABELS[g.role_ceiling ?? "owner"];

function describeGrant(g: Grant): string {
  if (g.kind === "app_publish") return `Publish ${g.app_name ?? "an app"}`;
  if (g.workspace_id === null) return `Every workspace, as ${roleOf(g)}`;
  return `${g.workspace_name ?? "A workspace"} as ${roleOf(g)}`;
}

/**
 * Grants as a sentence fragment someone can read without knowing the model:
 * "Every workspace, as Member", "Analytics as Viewer", "3 workspaces and
 * publish Store Ops".
 */
export function describeAccess(grants: Grant[]): AccessDescription {
  const active = live(grants);
  if (active.length === 0) {
    return {
      summary: grants.length > 0 ? "Access revoked" : "No access",
      details: []
    };
  }

  const details = active.map(describeGrant);
  const workspaces = active.filter((g) => g.kind === "workspace");
  const apps = active.filter((g) => g.kind === "app_publish");
  const orgWide = workspaces.find((g) => g.workspace_id === null);

  const parts: string[] = [];
  if (orgWide) parts.push(describeGrant(orgWide));
  else if (workspaces.length === 1) parts.push(describeGrant(workspaces[0]));
  else if (workspaces.length > 1) parts.push(`${workspaces.length} workspaces`);

  if (apps.length === 1) parts.push(describeGrant(apps[0]));
  else if (apps.length > 1) parts.push(`Publish ${apps.length} apps`);

  const [first, ...rest] = parts;
  const summary = [first, ...rest.map((p) => p.charAt(0).toLowerCase() + p.slice(1))].join(" and ");
  return { summary, details };
}
