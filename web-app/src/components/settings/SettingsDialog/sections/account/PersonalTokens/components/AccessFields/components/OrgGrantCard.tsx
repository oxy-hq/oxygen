import type React from "react";
import { useId } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import { cn } from "@/libs/shadcn/utils";
import type { TokenOptionOrg, TokenOptionWorkspace } from "@/types/apiToken";
import {
  type AccessAction,
  DEFAULT_CEILING,
  effectiveCeiling,
  type GrantTarget,
  isRevokedTarget,
  type OrgDraft
} from "../../../accessDraft";
import { CEILING_LABELS } from "../../../accessSummary";
import CeilingSelect from "./CeilingSelect";

interface Props {
  org: TokenOptionOrg;
  /** Absent until something in this org is picked. */
  draft?: OrgDraft;
  /** Targets an org took away from the token being edited: shown, and not pickable. */
  revoked: GrantTarget[];
  dispatch: React.Dispatch<AccessAction>;
}

interface WorkspaceRowProps {
  org: Pick<TokenOptionOrg, "org_id" | "org_name">;
  workspace: TokenOptionWorkspace;
  draft?: OrgDraft;
  /** The org removed this workspace from the token; it can't be added back. */
  locked: boolean;
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

/** One workspace: a checkbox, and once ticked, how much the token may do there. */
const WorkspaceRow: React.FC<WorkspaceRowProps> = ({ org, workspace, draft, locked, dispatch }) => {
  const id = useId();
  const orgId = org.org_id;
  const wide = !!draft?.wide;
  const ceiling = draft?.workspaces[workspace.workspace_id];
  // An org-wide grant still covers a workspace whose own grant was removed.
  const removed = locked && !wide;
  const picked = ceiling !== undefined && !removed;
  // A grant can't lift the token above its owner's own role: say so instead of implying it can.
  const capped = picked && !wide && effectiveCeiling(ceiling, workspace.role) !== ceiling;

  return (
    <li
      className='flex min-h-10 items-center gap-2 px-3 py-1'
      data-testid='account-token-workspace'
      data-workspace-name={workspace.name}
    >
      <Checkbox
        id={id}
        // Covered by the org-wide grant: shown ticked, and not individually editable.
        checked={wide || picked}
        disabled={wide || removed}
        onCheckedChange={(on) =>
          dispatch({
            type: "set_workspace",
            orgId,
            workspaceId: workspace.workspace_id,
            on: on === true
          })
        }
        data-testid='account-token-workspace-checkbox'
      />
      <label htmlFor={id} className='min-w-0 flex-1 cursor-pointer truncate text-xs'>
        {workspace.name}
        {removed && <RemovedNote orgName={org.org_name} />}
        {capped && (
          <span className='ml-2 text-muted-foreground'>
            capped at {CEILING_LABELS[workspace.role]}, your role here
          </span>
        )}
      </label>
      {picked && !wide && (
        <CeilingSelect
          value={ceiling}
          onChange={(next) =>
            dispatch({
              type: "set_workspace_ceiling",
              orgId,
              workspaceId: workspace.workspace_id,
              ceiling: next
            })
          }
          label={`Access to ${workspace.name}`}
          testId='account-token-workspace-ceiling'
        />
      )}
    </li>
  );
};

/** One org in the picker: its workspaces, and the option to cover all of them, future ones too. */
const OrgGrantCard: React.FC<Props> = ({ org, draft, revoked, dispatch }) => {
  const wideId = useId();
  // The org ended an org-wide grant: that one can't come back, single workspaces still can.
  const wideRemoved = isRevokedTarget({ revoked }, org.org_id, null);
  const wide = !!draft?.wide && !wideRemoved;

  return (
    <div
      className='overflow-hidden rounded-md border'
      data-testid='account-token-org'
      data-org-name={org.org_name}
    >
      <div className='flex flex-wrap items-center gap-x-3 gap-y-1.5 border-b bg-muted/40 px-3 py-2'>
        <span className='min-w-0 truncate font-medium text-xs'>{org.org_name}</span>
        {org.via === "partner" && (
          <Badge variant='outline' className='font-normal'>
            Partner
          </Badge>
        )}
        <div className='ml-auto flex items-center gap-2'>
          <Checkbox
            id={wideId}
            checked={wide}
            disabled={wideRemoved}
            onCheckedChange={(on) =>
              dispatch({ type: "set_org_wide", orgId: org.org_id, on: on === true })
            }
            data-testid='account-token-org-wide-checkbox'
          />
          <label htmlFor={wideId} className='cursor-pointer text-xs'>
            All workspaces in this org, including future ones
            {wideRemoved && <RemovedNote orgName={org.org_name} />}
          </label>
          {wide && (
            <CeilingSelect
              value={draft?.wideCeiling ?? DEFAULT_CEILING}
              onChange={(ceiling) =>
                dispatch({ type: "set_org_ceiling", orgId: org.org_id, ceiling })
              }
              label={`Access to every workspace in ${org.org_name}`}
              testId='account-token-org-ceiling'
            />
          )}
        </div>
      </div>
      {org.workspaces.length === 0 ? (
        <p className='px-3 py-2 text-muted-foreground text-xs'>No workspaces yet.</p>
      ) : (
        <ul className={cn("divide-y", wide && "opacity-60")}>
          {org.workspaces.map((workspace) => (
            <WorkspaceRow
              key={workspace.workspace_id}
              org={org}
              workspace={workspace}
              draft={draft}
              locked={isRevokedTarget({ revoked }, org.org_id, workspace.workspace_id)}
              dispatch={dispatch}
            />
          ))}
        </ul>
      )}
    </div>
  );
};

export default OrgGrantCard;
