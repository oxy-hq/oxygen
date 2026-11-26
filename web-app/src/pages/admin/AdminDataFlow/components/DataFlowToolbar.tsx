import { Crosshair, X } from "lucide-react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import type { DataFlowLevel } from "@/types/dataFlow";
import type { DataFlowParams } from "../useDataFlowParams";
import { OrgPicker } from "./OrgPicker";

const ALL_WORKSPACES = "__all__";
const DEPTHS = [1, 2, 3, 4, 5, 6];

/**
 * The page's controls, all of them URL state: the org, a workspace (only when the
 * org has more than one), schemas or tables, and — while focused — the depth and a
 * way out of the focus.
 */
export const DataFlowToolbar = ({
  params,
  orgName,
  workspaces,
  focusLabel,
  onChange
}: {
  params: DataFlowParams;
  orgName: string | null;
  workspaces: { id: string; name: string }[];
  focusLabel: string | null;
  onChange: (patch: Partial<DataFlowParams>) => void;
}) => (
  <div className='flex flex-wrap items-center gap-2' data-testid='admin-data-flow-toolbar'>
    <OrgPicker
      value={params.org ? { id: params.org, name: orgName } : null}
      onChange={(org) => onChange({ org: org.id, workspace: null, focus: null })}
    />
    {params.org && workspaces.length > 1 && (
      <Select
        value={params.workspace ?? ALL_WORKSPACES}
        onValueChange={(v) => onChange({ workspace: v === ALL_WORKSPACES ? null : v, focus: null })}
      >
        <SelectTrigger size='sm' className='w-48 text-xs' data-testid='admin-data-flow-workspace'>
          <SelectValue />
        </SelectTrigger>
        <SelectContent className={DESIGN_REFERENCE}>
          <SelectItem value={ALL_WORKSPACES}>All workspaces</SelectItem>
          {workspaces.map((w) => (
            <SelectItem key={w.id} value={w.id}>
              {w.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    )}
    {params.org && (
      <ToggleGroup
        type='single'
        size='sm'
        variant='outline'
        value={params.level}
        onValueChange={(v) => v && onChange({ level: v as DataFlowLevel })}
        data-testid='admin-data-flow-level'
      >
        <ToggleGroupItem value='schema' className='px-2.5 text-xs'>
          Schemas
        </ToggleGroupItem>
        <ToggleGroupItem value='table' className='px-2.5 text-xs'>
          Tables
        </ToggleGroupItem>
      </ToggleGroup>
    )}
    {params.focus && (
      <div
        className='flex items-center gap-1.5 border border-border px-2 py-1 text-xs'
        data-testid='admin-data-flow-focus'
      >
        <Crosshair className='size-3 text-muted-foreground' />
        <span className='max-w-56 truncate font-medium' title={params.focus}>
          {focusLabel ?? params.focus}
        </span>
        <Select value={String(params.depth)} onValueChange={(v) => onChange({ depth: Number(v) })}>
          <SelectTrigger
            size='sm'
            className='h-6 w-24 px-2 text-xs'
            data-testid='admin-data-flow-depth'
          >
            <SelectValue />
          </SelectTrigger>
          <SelectContent className={DESIGN_REFERENCE}>
            {DEPTHS.map((d) => (
              <SelectItem key={d} value={String(d)}>
                {d} {d === 1 ? "hop" : "hops"}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
        <Button
          size='icon'
          variant='ghost'
          className='size-5'
          aria-label='Clear focus'
          onClick={() => onChange({ focus: null })}
        >
          <X className='size-3' />
        </Button>
      </div>
    )}
  </div>
);
