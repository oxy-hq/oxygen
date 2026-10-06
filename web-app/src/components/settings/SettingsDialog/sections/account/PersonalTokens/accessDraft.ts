import type {
  GrantInput,
  RoleCeiling,
  Token,
  TokenAccessInput,
  TokenOptions
} from "@/types/apiToken";
import { CEILINGS } from "./accessSummary";

/** What the picker holds for one org while the dialog is open. */
export interface OrgDraft {
  /** Every workspace in the org, including ones created later. */
  wide: boolean;
  wideCeiling: RoleCeiling;
  /** Picked workspaces → ceiling. Kept while `wide` is on, so turning it off restores the picks. */
  workspaces: Record<string, RoleCeiling>;
}

/** One thing a grant can be on: a workspace, or (`null`) every workspace in the org. */
export interface GrantTarget {
  orgId: string;
  workspaceId: string | null;
}

export interface AccessDraft {
  mode: "all" | "selected";
  platform: boolean;
  partner: boolean;
  /** Only orgs with something picked. */
  orgs: Record<string, OrgDraft>;
  /**
   * Grants the picker can't edit (app-publish). `PATCH` replaces the whole set, so they are
   * carried through untouched rather than silently dropped.
   */
  kept: GrantInput[];
  /**
   * Targets an org took away from this token. The server never grants one of these back (asking
   * again is silently not granted), so the picker shows them locked rather than offering a tick
   * that would do nothing. Always empty for a new token.
   */
  revoked: GrantTarget[];
}

/** The contract's default: the token can do whatever its owner can in that workspace. */
export const DEFAULT_CEILING: RoleCeiling = "owner";

export const emptyDraft = (): AccessDraft => ({
  mode: "all",
  platform: false,
  partner: false,
  orgs: {},
  kept: [],
  revoked: []
});

/** Whether the org took this exact target away from the token being edited. */
export const isRevokedTarget = (
  draft: Pick<AccessDraft, "revoked">,
  orgId: string,
  workspaceId: string | null
): boolean =>
  draft.revoked.some((target) => target.orgId === orgId && target.workspaceId === workspaceId);

export type AccessAction =
  | { type: "set_mode"; mode: AccessDraft["mode"] }
  | { type: "set_standing"; flag: "platform" | "partner"; on: boolean }
  | { type: "set_org_wide"; orgId: string; on: boolean }
  | { type: "set_org_ceiling"; orgId: string; ceiling: RoleCeiling }
  | { type: "set_workspace"; orgId: string; workspaceId: string; on: boolean }
  | { type: "set_workspace_ceiling"; orgId: string; workspaceId: string; ceiling: RoleCeiling }
  | { type: "reset"; draft: AccessDraft };

const emptyOrg = (): OrgDraft => ({ wide: false, wideCeiling: DEFAULT_CEILING, workspaces: {} });

const hasPicks = (org: OrgDraft): boolean => org.wide || Object.keys(org.workspaces).length > 0;

/** Write one org back, dropping it once nothing in it is picked. */
const withOrg = (draft: AccessDraft, orgId: string, org: OrgDraft): AccessDraft => {
  const { [orgId]: _removed, ...rest } = draft.orgs;
  return { ...draft, orgs: hasPicks(org) ? { ...rest, [orgId]: org } : rest };
};

export const accessReducer = (draft: AccessDraft, action: AccessAction): AccessDraft => {
  switch (action.type) {
    case "reset":
      return action.draft;
    case "set_mode":
      // The picks survive a round trip to "All access" and back.
      return { ...draft, mode: action.mode };
    case "set_standing":
      return { ...draft, [action.flag]: action.on };
    case "set_org_wide": {
      const org = draft.orgs[action.orgId] ?? emptyOrg();
      return withOrg(draft, action.orgId, { ...org, wide: action.on });
    }
    case "set_org_ceiling": {
      const org = draft.orgs[action.orgId] ?? emptyOrg();
      return withOrg(draft, action.orgId, { ...org, wideCeiling: action.ceiling });
    }
    case "set_workspace": {
      const org = draft.orgs[action.orgId] ?? emptyOrg();
      const { [action.workspaceId]: current, ...others } = org.workspaces;
      const workspaces = action.on
        ? { ...others, [action.workspaceId]: current ?? DEFAULT_CEILING }
        : others;
      return withOrg(draft, action.orgId, { ...org, workspaces });
    }
    case "set_workspace_ceiling": {
      const org = draft.orgs[action.orgId];
      // A ceiling for a workspace that isn't picked has nothing to apply to.
      if (!org || !(action.workspaceId in org.workspaces)) return draft;
      return withOrg(draft, action.orgId, {
        ...org,
        workspaces: { ...org.workspaces, [action.workspaceId]: action.ceiling }
      });
    }
  }
};

/** The picker's state as the grants a create or update body carries. */
export const grantsFromDraft = (draft: AccessDraft): GrantInput[] => {
  const picked = Object.entries(draft.orgs).flatMap(([orgId, org]): GrantInput[] =>
    org.wide
      ? // An org-wide grant already covers every workspace: the individual picks would be noise.
        [{ kind: "workspace", org_id: orgId, workspace_id: null, role_ceiling: org.wideCeiling }]
      : Object.entries(org.workspaces).map(([workspaceId, ceiling]) => ({
          kind: "workspace" as const,
          org_id: orgId,
          workspace_id: workspaceId,
          role_ceiling: ceiling
        }))
  );
  // The picker locks these; this keeps a stray one out of the body all the same, where it would
  // otherwise count as a grant and then quietly not be given.
  const grantable = picked.filter(
    (grant) => !isRevokedTarget(draft, grant.org_id ?? "", grant.workspace_id ?? null)
  );
  return [...grantable, ...draft.kept];
};

/**
 * The access half of a create or update body. Every field is sent, so switching an existing
 * token back to all access also clears its grants.
 *
 * A standing flag is sent only while the caller still holds that standing (`holds`). A token
 * edited after its owner lost staff or partner standing has a checkbox nobody can see or clear;
 * re-sending the flag would fail the whole edit with `standing_required`.
 */
export const accessInputFromDraft = (
  draft: AccessDraft,
  holds: Pick<TokenOptions, "can_platform" | "can_partner"> = {
    can_platform: true,
    can_partner: true
  }
): TokenAccessInput => ({
  all_access: draft.mode === "all",
  platform: draft.platform && holds.can_platform,
  partner: draft.partner && holds.can_partner,
  grants: draft.mode === "all" ? [] : grantsFromDraft(draft)
});

/**
 * The picker's starting state for Edit access. Grants an org revoked are not re-requested: they
 * are recorded as locked targets instead.
 */
export const draftFromToken = (
  token: Pick<Token, "all_access" | "platform" | "partner" | "grants">
): AccessDraft => {
  const draft: AccessDraft = {
    ...emptyDraft(),
    mode: token.all_access ? "all" : "selected",
    platform: token.platform,
    partner: token.partner
  };
  for (const grant of token.grants ?? []) {
    if (grant.revoked_at) {
      if (grant.kind === "workspace") {
        draft.revoked.push({ orgId: grant.org_id, workspaceId: grant.workspace_id });
      }
      continue;
    }
    if (grant.kind === "app_publish") {
      draft.kept.push({
        kind: "app_publish",
        org_id: grant.org_id,
        app_id: grant.app_id ?? undefined
      });
      continue;
    }
    // Not a workspace grant, and not one this picker carries through: an `app_sandbox` grant
    // belongs to a sandbox agent token, which has no Edit access. Its `workspace_id` is `null`,
    // so reading it below would turn it into "every workspace in the org".
    if (grant.kind !== "workspace") continue;
    const org = draft.orgs[grant.org_id] ?? emptyOrg();
    const ceiling = grant.role_ceiling ?? DEFAULT_CEILING;
    draft.orgs[grant.org_id] =
      grant.workspace_id === null
        ? { ...org, wide: true, wideCeiling: ceiling }
        : { ...org, workspaces: { ...org.workspaces, [grant.workspace_id]: ceiling } };
  }
  return draft;
};

/** Why the draft can't be saved yet, or `null`. The server answers 400 to the same thing. */
export const draftProblem = (draft: AccessDraft): string | null =>
  draft.mode === "selected" && grantsFromDraft(draft).length === 0
    ? "Select at least one workspace, or switch to all access."
    : null;

export interface LifetimeCap {
  days: number;
  /** The org whose policy is the tightest, to name in the hint. */
  orgName: string;
}

/**
 * The shortest max lifetime among the orgs a narrowed token names. An all-access token is not
 * capped here: an org that limits lifetimes blocks it instead (`blocked_orgs`).
 */
export const lifetimeCap = (
  draft: AccessDraft,
  options: Pick<TokenOptions, "orgs"> | undefined
): LifetimeCap | null => {
  if (draft.mode !== "selected" || !options) return null;
  let cap: LifetimeCap | null = null;
  for (const org of options.orgs) {
    const days = org.policy?.max_lifetime_days;
    if (!(org.org_id in draft.orgs) || days === null || days === undefined) continue;
    if (!cap || days < cap.days) cap = { days, orgName: org.org_name };
  }
  return cap;
};

/** Orgs whose policy turns all-access tokens away: such a token would not work there. */
export const orgsRefusingAllAccess = (options: Pick<TokenOptions, "orgs"> | undefined): string[] =>
  (options?.orgs ?? [])
    .filter((org) => org.policy?.allow_all_access_tokens === false)
    .map((org) => org.org_name);

/**
 * Picked orgs the caller reaches only as a partner, while the token leaves partner access out.
 * Without it the grant has no authority behind it.
 */
export const partnerOrgsWithoutStanding = (
  draft: AccessDraft,
  options: Pick<TokenOptions, "orgs"> | undefined
): string[] =>
  draft.mode !== "selected" || draft.partner
    ? []
    : (options?.orgs ?? [])
        .filter((org) => org.via === "partner" && org.org_id in draft.orgs)
        .map((org) => org.org_name);

/** The lower of the grant's ceiling and the caller's own role: what the token really gets. */
export const effectiveCeiling = (ceiling: RoleCeiling, role: RoleCeiling): RoleCeiling =>
  CEILINGS.indexOf(ceiling) <= CEILINGS.indexOf(role) ? ceiling : role;
