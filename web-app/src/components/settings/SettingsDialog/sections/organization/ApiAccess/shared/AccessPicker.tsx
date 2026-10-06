import { type ReactNode, useId } from "react";
import { Checkbox } from "@/components/ui/shadcn/checkbox";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import type { RoleCeiling } from "@/types/apiToken";
import type { ServiceAccountRole } from "@/types/orgApiAccess";
import {
  type AccessDraft,
  ceilingOptions,
  ROLE_LABELS,
  setWorkspaceCeiling,
  toggleApp,
  toggleWorkspace
} from "../utils/grants";
import { ChoiceRow } from "./FormField";

export interface PickableItem {
  id: string;
  name: string;
}

/** A list the picker reads from another query, with that query's state. */
export interface PickableList {
  items: PickableItem[];
  isPending: boolean;
  isError: boolean;
}

interface AccessPickerProps {
  value: AccessDraft;
  onChange: (next: AccessDraft) => void;
  /** Sets the ceilings on offer and the role the whole-org option names. */
  accountRole: ServiceAccountRole;
  workspaces: PickableList;
  /** Pass to also offer "may publish this app". Tokens don't; policies do. */
  apps?: PickableList;
  /** Prefix for testids, so two pickers on screen stay distinguishable. */
  testId: string;
}

/**
 * What a token or a trusted-access policy may reach: the whole organization at
 * the account's role, or named workspaces each with its own ceiling — and, for
 * a policy, the apps it may publish.
 */
export function AccessPicker({
  value,
  onChange,
  accountRole,
  workspaces,
  apps,
  testId
}: AccessPickerProps) {
  const group = useId();
  const role = ROLE_LABELS[accountRole];

  return (
    <div className='flex flex-col gap-2'>
      <fieldset className='flex flex-col gap-0.5 border-0 p-0'>
        <legend className='sr-only'>Workspaces</legend>
        <ChoiceRow
          group={group}
          checked={value.scope === "org"}
          onSelect={() => onChange({ ...value, scope: "org" })}
          label='The whole organization'
          hint={`Every workspace, including ones created later, as ${role}.`}
          testId={`${testId}-scope-org`}
        />
        <ChoiceRow
          group={group}
          checked={value.scope === "selected"}
          onSelect={() => onChange({ ...value, scope: "selected" })}
          label='Selected workspaces'
          hint='Only the workspaces you pick, each capped at a role.'
          testId={`${testId}-scope-selected`}
        />
      </fieldset>

      {value.scope === "selected" && (
        <Checklist list={workspaces} what='workspaces' testId={`${testId}-workspaces`}>
          {(workspace) => {
            const picked = value.workspaces.find((w) => w.workspace_id === workspace.id);
            return (
              <WorkspaceRow
                key={workspace.id}
                workspace={workspace}
                ceiling={picked?.role_ceiling}
                accountRole={accountRole}
                onToggle={() => onChange(toggleWorkspace(value, workspace.id, accountRole))}
                onCeiling={(ceiling) => onChange(setWorkspaceCeiling(value, workspace.id, ceiling))}
                testId={`${testId}-workspace`}
              />
            );
          }}
        </Checklist>
      )}

      {apps && (
        <div className='flex flex-col gap-1.5 pt-1'>
          <p className='font-medium text-xs'>Apps it may publish</p>
          <Checklist list={apps} what='apps' testId={`${testId}-apps`}>
            {(app) => (
              <AppRow
                key={app.id}
                app={app}
                checked={value.appIds.includes(app.id)}
                onToggle={() => onChange(toggleApp(value, app.id))}
                testId={`${testId}-app`}
              />
            )}
          </Checklist>
        </div>
      )}
    </div>
  );
}

function AppRow({
  app,
  checked,
  onToggle,
  testId
}: {
  app: PickableItem;
  checked: boolean;
  onToggle: () => void;
  testId: string;
}) {
  const checkboxId = useId();
  return (
    <div
      className='flex items-center gap-2 px-3 py-1.5 text-xs'
      data-testid={testId}
      data-app-name={app.name}
    >
      <Checkbox id={checkboxId} checked={checked} onCheckedChange={onToggle} />
      <label htmlFor={checkboxId} className='min-w-0 flex-1 cursor-pointer truncate py-1'>
        Publish {app.name}
      </label>
    </div>
  );
}

function WorkspaceRow({
  workspace,
  ceiling,
  accountRole,
  onToggle,
  onCeiling,
  testId
}: {
  workspace: PickableItem;
  /** Set when the workspace is picked. */
  ceiling: RoleCeiling | undefined;
  accountRole: ServiceAccountRole;
  onToggle: () => void;
  onCeiling: (ceiling: RoleCeiling) => void;
  testId: string;
}) {
  const checkboxId = useId();
  return (
    <div
      className='flex items-center gap-2 px-3 py-1.5 text-xs'
      data-testid={testId}
      data-workspace-name={workspace.name}
    >
      <Checkbox id={checkboxId} checked={ceiling !== undefined} onCheckedChange={onToggle} />
      <label htmlFor={checkboxId} className='min-w-0 flex-1 cursor-pointer truncate py-1'>
        {workspace.name}
      </label>
      {ceiling !== undefined && (
        <Select value={ceiling} onValueChange={(v) => onCeiling(v as RoleCeiling)}>
          <SelectTrigger
            size='sm'
            className='h-7 w-28 text-xs'
            aria-label={`Role in ${workspace.name}`}
            data-testid={`${testId}-ceiling`}
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            {ceilingOptions(accountRole).map((option) => (
              <SelectItem key={option} value={option} className='text-xs'>
                {ROLE_LABELS[option]}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      )}
    </div>
  );
}

/** A bordered, scrollable list of pickable rows, with its own quiet states. */
function Checklist({
  list,
  what,
  testId,
  children
}: {
  list: PickableList;
  what: string;
  testId: string;
  children: (item: PickableItem) => ReactNode;
}) {
  if (list.isPending) {
    return (
      <div className='flex flex-col gap-1.5 rounded-md border p-2'>
        <span className='sr-only'>Loading {what}</span>
        <Skeleton className='h-6 w-full' />
        <Skeleton className='h-6 w-3/4' />
      </div>
    );
  }
  if (list.isError) {
    return (
      <p className='rounded-md border p-3 text-muted-foreground text-xs'>
        Couldn't load this organization's {what}. Close this and try again.
      </p>
    );
  }
  if (list.items.length === 0) {
    return (
      <p className='rounded-md border p-3 text-muted-foreground text-xs'>
        This organization has no {what} yet.
      </p>
    );
  }
  return (
    <div className='max-h-44 divide-y overflow-y-auto rounded-md border' data-testid={testId}>
      {list.items.map(children)}
    </div>
  );
}
