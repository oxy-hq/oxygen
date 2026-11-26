import { Cog, Search, X } from "lucide-react";
import type React from "react";
import { useEffect, useRef, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { cn } from "@/libs/shadcn/utils";

export interface RunFilters {
  status: string;
  type: string;
  source: string;
  range: string;
  search: string;
  /** Whether to include system-managed daemon runs in the feed. */
  includeSystem: boolean;
}

export const DEFAULT_RUN_FILTERS: RunFilters = {
  status: "all",
  type: "all",
  source: "all",
  range: "24h",
  search: "",
  includeSystem: false
};

const isDefaultFilters = (f: RunFilters): boolean =>
  f.status === "all" &&
  f.type === "all" &&
  f.source === "all" &&
  f.search === "" &&
  f.includeSystem === false;

const FilterSelect = <T extends string>({
  value,
  onChange,
  options,
  width
}: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: string }[];
  width: string;
}) => (
  <Select value={value} onValueChange={(v) => onChange(v as T)}>
    <SelectTrigger className={width} size='sm'>
      <SelectValue />
    </SelectTrigger>
    <SelectContent className={DESIGN_REFERENCE}>
      {options.map((o) => (
        <SelectItem key={o.value} value={o.value}>
          {o.label}
        </SelectItem>
      ))}
    </SelectContent>
  </Select>
);

/**
 * The Runs filter bar — the heavy-lifter of this tab. "Failed in the last
 * hour" should be one or two clicks; the filter state lives in the URL so a
 * filtered view is shareable.
 */
/** Debounce window for the search input. Short enough to feel
 *  immediate, long enough that a normal typing cadence (~120 ms per
 *  keystroke) only fires one URL push at the end of a word. */
const SEARCH_DEBOUNCE_MS = 250;

export const RunsFilterBar: React.FC<{
  value: RunFilters;
  onChange: (next: RunFilters) => void;
}> = ({ value, onChange }) => {
  const patch = (p: Partial<RunFilters>) => onChange({ ...value, ...p });

  // Local search state debounce-pushed to the URL. Previously the
  // search input wrote on every keystroke, which re-rendered the
  // whole Runs page (URL change → readFilters → new filters obj →
  // useRunsModel refetch → full rerender) and the input dropped
  // characters under fast typing. Keeping the buffer local + only
  // pushing after a quiet period keeps typing snappy.
  const [searchDraft, setSearchDraft] = useState(value.search);

  // Re-sync the local draft when the parent's `value.search` changes
  // for reasons other than this input — e.g. the user hit "Clear"
  // filters, or navigated with a URL containing `?search=...`.
  const lastPushedRef = useRef(value.search);
  useEffect(() => {
    if (value.search !== lastPushedRef.current) {
      setSearchDraft(value.search);
      lastPushedRef.current = value.search;
    }
  }, [value.search]);

  // Keep refs to the latest `value` + `onChange` so the debounce
  // effect can read them without depending on them. Without this
  // indirection the deps array would have to include `value` and
  // `onChange`, and each parent re-render (Coordinator-live SSE
  // tick, URL change, etc.) would cancel + restart the timer —
  // defeating the entire debounce.
  const valueRef = useRef(value);
  const onChangeRef = useRef(onChange);
  useEffect(() => {
    valueRef.current = value;
    onChangeRef.current = onChange;
  });

  // Debounce-push the draft up to the parent. The cleanup cancels a
  // pending push if the user types another character before the
  // window elapses. Deps are `[searchDraft]` *by design*.
  useEffect(() => {
    if (searchDraft === valueRef.current.search) return;
    const t = setTimeout(() => {
      lastPushedRef.current = searchDraft;
      onChangeRef.current({ ...valueRef.current, search: searchDraft });
    }, SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [searchDraft]);

  return (
    <div className='flex flex-wrap items-center gap-2'>
      <FilterSelect
        value={value.status}
        onChange={(status) => patch({ status })}
        width='w-32'
        options={[
          { value: "all", label: "All statuses" },
          { value: "running", label: "Running" },
          { value: "suspended", label: "Suspended" },
          { value: "done", label: "Succeeded" },
          { value: "failed", label: "Failed" },
          { value: "cancelled", label: "Cancelled" }
        ]}
      />
      <FilterSelect
        value={value.type}
        onChange={(type) => patch({ type })}
        width='w-28'
        options={[
          { value: "all", label: "All types" },
          { value: "agent", label: "Agent" },
          { value: "dag", label: "DAG" },
          { value: "elt", label: "ELT" },
          { value: "monitor", label: "Monitor" }
        ]}
      />
      <FilterSelect
        value={value.source}
        onChange={(source) => patch({ source })}
        width='w-36'
        options={[
          { value: "all", label: "All sources" },
          { value: "analytics", label: "Analytics" },
          { value: "builder", label: "Builder" },
          { value: "workflow", label: "Automation" },
          { value: "airway", label: "Airway" },
          { value: "coordinator", label: "Coordinator" },
          { value: "monitor_scan", label: "Monitor" }
        ]}
      />
      <FilterSelect
        value={value.range}
        onChange={(range) => patch({ range })}
        width='w-28'
        options={[
          { value: "1h", label: "Last 1h" },
          { value: "24h", label: "Last 24h" },
          { value: "7d", label: "Last 7d" },
          { value: "all", label: "All time" }
        ]}
      />
      <div className='relative ml-auto'>
        <Search className='absolute top-1/2 left-2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground' />
        <Input
          value={searchDraft}
          onChange={(e) => setSearchDraft(e.target.value)}
          onKeyDown={(e) => {
            // Enter commits immediately, Escape clears.
            if (e.key === "Enter") {
              lastPushedRef.current = searchDraft;
              onChange({ ...value, search: searchDraft });
            } else if (e.key === "Escape") {
              setSearchDraft("");
              lastPushedRef.current = "";
              onChange({ ...value, search: "" });
            }
          }}
          placeholder='Search runs'
          className='h-8 w-52 pl-7'
        />
      </div>
      <Button
        variant='outline'
        size='sm'
        className={cn(
          "h-8",
          value.includeSystem && "border-muted-foreground/40 bg-muted text-foreground"
        )}
        onClick={() => patch({ includeSystem: !value.includeSystem })}
        title='System-managed daemon runs (preagg_cycle, etc.)'
      >
        <Cog className='h-3.5 w-3.5' />
        {value.includeSystem ? "System: on" : "System: off"}
      </Button>
      {!isDefaultFilters(value) && (
        <Button
          variant='ghost'
          size='sm'
          className='h-8'
          onClick={() => onChange({ ...DEFAULT_RUN_FILTERS, range: value.range })}
        >
          <X className='h-3.5 w-3.5' />
          Clear
        </Button>
      )}
    </div>
  );
};
