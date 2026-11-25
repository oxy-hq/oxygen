import { ChevronsDownUp, ChevronsUpDown, Search } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import { HIDEABLE_KINDS, type HideableKind } from "../dataFlowView";
import type { DataFlowParams } from "../useDataFlowParams";

const SEARCH_DEBOUNCE_MS = 250;

/** A search box that writes the URL on a pause, so the canvas is not re-laid out per keystroke. */
const FlowSearch = ({ value, onChange }: { value: string; onChange: (q: string) => void }) => {
  const [draft, setDraft] = useState(value);
  // The latest callback, so a parent re-render does not restart the pause.
  const commit = useRef(onChange);
  commit.current = onChange;
  useEffect(() => setDraft(value), [value]);
  useEffect(() => {
    if (draft === value) return;
    const id = window.setTimeout(() => commit.current(draft), SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(id);
  }, [draft, value]);
  return (
    <div className='relative'>
      <Search className='-translate-y-1/2 absolute top-1/2 left-2 size-3 text-muted-foreground' />
      <Input
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        placeholder='Find a source, pipeline, schema or app…'
        className='h-8 w-64 pl-7 text-xs'
        data-testid='admin-data-flow-search'
      />
    </div>
  );
};

/**
 * What the canvas draws of the answer: the kinds shown, a search that keeps its
 * matches and their neighbours, and folding every group open or shut. None of it
 * goes to the server.
 */
export const DataFlowViewBar = ({
  params,
  foldable,
  onChange
}: {
  params: DataFlowParams;
  /** How many groups the current view can fold; the fold buttons show only when some can. */
  foldable: number;
  onChange: (patch: Partial<DataFlowParams>) => void;
}) => {
  const shown = HIDEABLE_KINDS.map((k) => k.kind).filter((k) => !params.hide.includes(k));
  return (
    <div className='flex flex-wrap items-center gap-2' data-testid='admin-data-flow-viewbar'>
      <FlowSearch value={params.q} onChange={(q) => onChange({ q })} />
      <ToggleGroup
        type='multiple'
        size='sm'
        variant='outline'
        value={shown}
        onValueChange={(v) =>
          onChange({
            hide: HIDEABLE_KINDS.map((k) => k.kind).filter((k): k is HideableKind => !v.includes(k))
          })
        }
        data-testid='admin-data-flow-kinds'
      >
        {HIDEABLE_KINDS.map((k) => (
          <ToggleGroupItem
            key={k.kind}
            value={k.kind}
            className='px-2.5 text-xs'
            data-testid={`admin-data-flow-kind-${k.kind}`}
          >
            {k.label}
          </ToggleGroupItem>
        ))}
      </ToggleGroup>
      {foldable > 0 && (
        <div className='flex items-center gap-1'>
          <Button
            size='sm'
            variant='ghost'
            className='h-8 text-xs'
            onClick={() => onChange({ open: "all" })}
            disabled={params.open === "all"}
            data-testid='admin-data-flow-expand-all'
          >
            <ChevronsUpDown className='size-3' /> Expand all
          </Button>
          <Button
            size='sm'
            variant='ghost'
            className='h-8 text-xs'
            onClick={() => onChange({ open: [] })}
            disabled={params.open !== "all" && params.open.length === 0}
            data-testid='admin-data-flow-collapse-all'
          >
            <ChevronsDownUp className='size-3' /> Collapse all
          </Button>
        </div>
      )}
    </div>
  );
};
