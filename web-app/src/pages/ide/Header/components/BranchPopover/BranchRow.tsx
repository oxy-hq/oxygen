import { GitBranch, Trash2 } from "lucide-react";
import { CanWorkspaceAdmin } from "@/components/auth/Can";
import { CommandItem } from "@/components/ui/shadcn/command";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { cn } from "@/libs/shadcn/utils";

type BranchOrigin = "local_only" | "remote_only" | "both";

export interface BranchRowData {
  name: string;
  /** Shown instead of `name` when the row is not a branch — the workspace's
   * detached working copy, whose `name` is a `HEAD@<sha>` label. */
  label?: string;
  origin?: BranchOrigin;
  showActiveBadge?: boolean;
  canDelete?: boolean;
}

interface Props {
  row: BranchRowData;
  isActive: boolean;
  isSwitchingThis: boolean;
  isSwitchingOther: boolean;
  onSelect: () => void;
  onDelete?: () => void;
}

export function BranchRow({
  row,
  isActive,
  isSwitchingThis,
  isSwitchingOther,
  onSelect,
  onDelete
}: Props) {
  return (
    <CommandItem
      value={row.label ?? row.name}
      // Block all rows during a switch — a fast double-click would fire a
      // second (idempotent but churn-y) mutation.
      disabled={isSwitchingThis || isSwitchingOther}
      onSelect={onSelect}
      className={cn(
        "group flex cursor-pointer items-center gap-2.5 font-mono text-sm",
        isSwitchingOther && "opacity-50"
      )}
    >
      <span
        className={cn(
          "h-1.5 w-1.5 shrink-0 rounded-full transition-colors",
          isActive ? "bg-primary" : "bg-transparent group-aria-selected:bg-muted-foreground/25"
        )}
      />
      <span className='min-w-0 flex-1 truncate' title={row.label ?? row.name}>
        {row.label ?? row.name}
      </span>
      {isSwitchingThis && (
        <Spinner className='size-3 shrink-0 text-muted-foreground' aria-label='Switching…' />
      )}
      {row.origin === "remote_only" && (
        <span
          className='shrink-0 rounded bg-muted px-1.5 py-0.5 font-medium font-sans text-muted-foreground text-xs'
          title='Exists on origin only — switching will create a local tracking branch.'
        >
          remote
        </span>
      )}
      {row.origin === "local_only" && (
        <span
          className='shrink-0 rounded bg-muted px-1.5 py-0.5 font-medium font-sans text-muted-foreground text-xs'
          title='Local-only branch — never pushed to origin.'
        >
          local
        </span>
      )}
      {row.showActiveBadge && (
        <span className='shrink-0 font-sans text-muted-foreground/60 text-xs'>active</span>
      )}
      {isActive && <GitBranch className='h-3 w-3 shrink-0 text-primary' />}
      {onDelete && (
        <CanWorkspaceAdmin>
          <button
            type='button'
            className='ml-auto hidden items-center text-muted-foreground hover:text-destructive group-hover:flex'
            onClick={(e) => {
              e.stopPropagation();
              onDelete();
            }}
            title='Delete branch'
          >
            <Trash2 className='h-3 w-3' />
          </button>
        </CanWorkspaceAdmin>
      )}
    </CommandItem>
  );
}
