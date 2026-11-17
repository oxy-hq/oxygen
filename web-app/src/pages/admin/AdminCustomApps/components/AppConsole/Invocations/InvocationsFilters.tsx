import { RotateCw } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";

/** What the explorer is narrowed to. `""` is "any". All of it is sent to the
 *  server, which applies it before the cap. */
export interface InvocationFilters {
  fn: string;
  outcome: "all" | "failed";
  mode: string;
}

const SELECT = "h-7 rounded-md border border-input bg-background px-1.5 text-xs";

/** One dense row: function, outcome, mode, and when the window was read. */
export const InvocationsFilters = ({
  value,
  onChange,
  functions,
  modes,
  asOf,
  onRefresh,
  refreshing
}: {
  value: InvocationFilters;
  onChange: (next: InvocationFilters) => void;
  /** Function names in the build the environment serves. */
  functions: string[];
  /** Modes in the data. */
  modes: string[];
  /** The end of the window, as read. */
  asOf: Date;
  onRefresh: () => void;
  refreshing: boolean;
}) => {
  // A function the selection names but the build no longer has (it was renamed, or
  // only ran on an older build) stays selectable rather than silently vanishing.
  const fnOptions =
    value.fn && !functions.includes(value.fn) ? [value.fn, ...functions] : functions;
  return (
    <div
      className='flex flex-wrap items-center gap-2 text-xs'
      data-testid='admin-app-invocations-filters'
    >
      <select
        value={value.fn}
        onChange={(e) => onChange({ ...value, fn: e.target.value })}
        className={SELECT}
        aria-label='Function'
        data-testid='admin-app-invocations-filter-function'
      >
        <option value=''>All functions</option>
        {fnOptions.map((name) => (
          <option key={name} value={name}>
            {name}
          </option>
        ))}
      </select>
      <ToggleGroup
        type='single'
        size='sm'
        value={value.outcome}
        onValueChange={(v) =>
          v && onChange({ ...value, outcome: v as InvocationFilters["outcome"] })
        }
        className='gap-0 rounded-md border border-border/60 p-0.5'
        aria-label='Outcome'
        data-testid='admin-app-invocations-filter-outcome'
      >
        <ToggleGroupItem value='all' className='h-6 px-2 text-xs data-[state=on]:bg-muted'>
          All
        </ToggleGroupItem>
        <ToggleGroupItem value='failed' className='h-6 px-2 text-xs data-[state=on]:bg-muted'>
          Failed
        </ToggleGroupItem>
      </ToggleGroup>
      <select
        value={value.mode}
        onChange={(e) => onChange({ ...value, mode: e.target.value })}
        className={SELECT}
        aria-label='Mode'
        data-testid='admin-app-invocations-filter-mode'
      >
        <option value=''>Any mode</option>
        {modes.map((mode) => (
          <option key={mode} value={mode}>
            {mode}
          </option>
        ))}
      </select>
      <span className='ml-auto flex items-center gap-1 text-muted-foreground'>
        <span title={asOf.toLocaleString()}>
          as of{" "}
          {asOf.toLocaleTimeString(undefined, {
            hour: "2-digit",
            minute: "2-digit"
          })}
        </span>
        <Button
          variant='ghost'
          size='sm'
          className='size-6 p-0'
          onClick={onRefresh}
          disabled={refreshing}
          aria-label='Read the window again, ending now'
          title='Read the window again, ending now'
          data-testid='admin-app-invocations-refresh'
        >
          <RotateCw className={refreshing ? "size-3 animate-spin" : "size-3"} />
        </Button>
      </span>
    </div>
  );
};
