import { Layers, List } from "lucide-react";

import { Input } from "@/components/ui/shadcn/input";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";

export type CompileView = "workspace" | "revisions";

const STATUSES = ["", "compiling", "ready", "failed"] as const;

/**
 * Segmented "By workspace | All revisions" toggle plus the shared search
 * + status filters. Search is a free-text box (workspace name/path/id in
 * the rollup view, workspace UUID in the flat view). Kept dense: one row,
 * h-8 controls, monospace search.
 */
export const CompileFilters = ({
  view,
  onViewChange,
  query,
  onQueryChange,
  status,
  onStatusChange,
  totalLabel
}: {
  view: CompileView;
  onViewChange: (v: CompileView) => void;
  query: string;
  onQueryChange: (v: string) => void;
  status: string;
  onStatusChange: (v: string) => void;
  totalLabel: string;
}) => (
  <div className='flex flex-wrap items-center justify-between gap-2'>
    <div className='flex flex-wrap items-center gap-2'>
      <ToggleGroup
        type='single'
        size='sm'
        value={view}
        onValueChange={(v) => v && onViewChange(v as CompileView)}
        className='gap-0 border border-border/60 bg-card p-0.5'
      >
        <ToggleGroupItem
          value='workspace'
          aria-label='By workspace'
          className='h-7 gap-1.5 px-2.5 text-xs data-[state=on]:bg-muted'
        >
          <Layers className='size-3.5' />
          By workspace
        </ToggleGroupItem>
        <ToggleGroupItem
          value='revisions'
          aria-label='All revisions'
          className='h-7 gap-1.5 px-2.5 text-xs data-[state=on]:bg-muted'
        >
          <List className='size-3.5' />
          All revisions
        </ToggleGroupItem>
      </ToggleGroup>

      <Input
        placeholder={view === "workspace" ? "Search name, path or ID" : "Filter by workspace UUID"}
        value={query}
        onChange={(e) => onQueryChange(e.target.value)}
        className='h-8 w-64 font-mono text-xs'
        aria-label='Search compiles'
      />

      <select
        value={status}
        onChange={(e) => onStatusChange(e.target.value)}
        className='h-8 border border-input bg-background px-2 text-xs'
        aria-label='Status filter'
      >
        {STATUSES.map((s) => (
          <option key={s || "any"} value={s}>
            {s || "any status"}
          </option>
        ))}
      </select>
    </div>

    <span className='font-medium text-[11px] text-muted-foreground tabular-nums'>{totalLabel}</span>
  </div>
);
