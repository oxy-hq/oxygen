import type React from "react";
import { useId } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import type { RoleCeiling, TokenOptionOrg } from "@/types/apiToken";
import {
  type AccessAction,
  DEFAULT_CEILING,
  effectiveCeiling,
  type GrantTarget,
  isRevokedTarget,
  type OrgDraft
} from "../../../accessDraft";
import { CEILING_LABELS, CEILINGS } from "../../../accessSummary";
import CeilingSelect from "./CeilingSelect";

interface Props {
  org: TokenOptionOrg;
  /** Absent until something in this org is picked. */
  draft?: OrgDraft;
  /** Targets an org took away from the token being edited: shown, and not pickable. */
  revoked: GrantTarget[];
  dispatch: React.Dispatch<AccessAction>;
}

/** Said beside a target its org took away, so the dead checkbox explains itself. */
const RemovedNote: React.FC<{ orgName: string }> = ({ orgName }) => (
  <span
    className='ml-2 text-muted-foreground'
    title={`${orgName} removed this from the token. It can't be added back to this token.`}
    data-testid='account-token-grant-removed'
  >
    removed by {orgName}
  </span>
);

/** The lowest of `roles`, the one that caps a grant over all of them; `fallback` when none. */
const lowestRole = (roles: RoleCeiling[], fallback: RoleCeiling): RoleCeiling =>
  roles.reduce<RoleCeiling | null>(
    (low, role) => (low === null || CEILINGS.indexOf(role) < CEILINGS.indexOf(low) ? role : low),
    null
  ) ?? fallback;

/**
 * One org in the picker: a checkbox, and once ticked, how much the token may do there.
 *
 * Reach is chosen by organization. A pick is stored as an org-wide grant (`workspace_id: null`):
 * an org has one workspace and no screen names it, so an org-wide grant is "this organization".
 *
 * A few orgs still have more than one workspace until they are merged (grain memo §7), and a
 * token made earlier may be narrowed to one of them. There the two differ, so a narrowed grant
 * stays narrowed: changing its access changes each narrowed grant's ceiling and never widens it,
 * the row says it covers only part of the org, and the cap is the lowest role it covers. Only an
 * org with one workspace — where the two reach the same data — turns a narrowed grant org-wide
 * when its access changes.
 */
const OrgGrantCard: React.FC<Props> = ({ org, draft, revoked, dispatch }) => {
  const id = useId();
  const single = org.workspaces.length <= 1;
  const narrowIds = Object.keys(draft?.workspaces ?? {});
  const narrowCeilings = Object.values(draft?.workspaces ?? {});
  // The draft holds only live grants, so a pick shown here is one the token still has.
  const wide = !!draft?.wide;
  const narrowed = !wide && narrowIds.length > 0;
  const picked = wide || narrowed;
  // What the org took away from this token. With one workspace, the org-wide grant and the one on
  // its workspace reach the same data, so either closes the org. With several, only taking away
  // the org-wide grant closes the tick, which is what ticking would add back.
  const removed = single
    ? revoked.some((target) => target.orgId === org.org_id)
    : isRevokedTarget({ revoked }, org.org_id, null);
  const ceiling = wide
    ? (draft?.wideCeiling ?? DEFAULT_CEILING)
    : (narrowCeilings[0] ?? DEFAULT_CEILING);
  // The caller's own role caps the grant. An org-wide grant meets every workspace's role, a
  // narrowed one only its own; the lowest is the one that caps.
  const covered = narrowed
    ? org.workspaces.filter((w) => narrowIds.includes(w.workspace_id))
    : org.workspaces;
  const role = lowestRole(
    covered.map((w) => w.role),
    org.role
  );
  // A grant can't lift the token above its owner's own role: say so instead of implying it can.
  const capped = picked && effectiveCeiling(ceiling, role) !== ceiling;

  const toggle = (on: boolean) => {
    dispatch({ type: "set_org_wide", orgId: org.org_id, on });
    if (on) return;
    for (const workspaceId of narrowIds) {
      dispatch({ type: "set_workspace", orgId: org.org_id, workspaceId, on: false });
    }
  };

  const setCeiling = (next: RoleCeiling) => {
    if (narrowed && !single) {
      for (const workspaceId of narrowIds) {
        dispatch({ type: "set_workspace_ceiling", orgId: org.org_id, workspaceId, ceiling: next });
      }
      return;
    }
    if (!wide) dispatch({ type: "set_org_wide", orgId: org.org_id, on: true });
    dispatch({ type: "set_org_ceiling", orgId: org.org_id, ceiling: next });
  };

  return (
    <div
      className='flex min-h-10 flex-wrap items-center gap-x-3 gap-y-1.5 rounded-md border px-3 py-1.5'
      data-testid='account-token-org'
      data-org-name={org.org_name}
    >
      <Checkbox
        id={id}
        checked={picked}
        // Untick a grant the token still holds, but never re-add one the org closed.
        disabled={removed && !picked}
        onCheckedChange={(on) => toggle(on === true)}
        data-testid='account-token-org-wide-checkbox'
      />
      <label htmlFor={id} className='min-w-0 flex-1 cursor-pointer truncate text-xs'>
        <span className='font-medium'>{org.org_name}</span>
        {removed && !picked && <RemovedNote orgName={org.org_name} />}
        {narrowed && !single && (
          <span
            className='ml-2 text-muted-foreground'
            title={`This token was limited to part of ${org.org_name} when it was made. Changing its access keeps that limit; untick and tick it again to give it all of ${org.org_name}.`}
            data-testid='account-token-org-partial'
          >
            only part of it
          </span>
        )}
        {capped && (
          <span className='ml-2 text-muted-foreground'>
            capped at {CEILING_LABELS[role]}, your role here
          </span>
        )}
      </label>
      {org.via === "partner" && (
        <Badge variant='outline' className='font-normal'>
          Partner
        </Badge>
      )}
      {picked && (
        <CeilingSelect
          value={ceiling}
          onChange={setCeiling}
          label={`Access to ${org.org_name}`}
          testId='account-token-org-ceiling'
        />
      )}
    </div>
  );
};

export default OrgGrantCard;
