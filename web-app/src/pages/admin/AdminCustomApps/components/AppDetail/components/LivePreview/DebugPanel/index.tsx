import { Activity, ChevronDown, ChevronUp, Trash2 } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { cn } from "@/libs/shadcn/utils";
import type { OxyRequestEntry } from "../useOxyRequestLog";
import { RequestDetail } from "./components/RequestDetail";
import { RequestRow } from "./components/RequestRow";

/**
 * Network-inspector drawer pinned to the bottom of the preview pane. Shows
 * the requests the previewed app made to oxy's API (captured by
 * `useOxyRequestLog`). Devtools-dense: mono rows, status-coloured, collapsible
 * to a thin bar so it costs no vertical space when not in use. Selecting a row
 * opens a master-detail pane with headers, payload, and response body.
 */
export const DebugPanel = ({
  entries,
  onClear,
  open,
  onToggle,
  available,
  appId
}: {
  entries: OxyRequestEntry[];
  onClear: () => void;
  open: boolean;
  onToggle: () => void;
  available: boolean;
  /** The previewed app: a function call's detail opens its invocation's calls. */
  appId?: string;
}) => {
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const selected = entries.find((e) => e.id === selectedId) ?? null;

  const handleClear = () => {
    setSelectedId(null);
    onClear();
  };

  if (!available) {
    return (
      <div className='flex items-center gap-2 border-sidebar-border/50 border-t bg-card px-3 py-1.5 text-muted-foreground text-xs'>
        <Activity className='h-3.5 w-3.5' />
        Request inspector unavailable — the preview is cross-origin.
      </div>
    );
  }

  if (!open) {
    return (
      <button
        type='button'
        onClick={onToggle}
        data-testid='admin-app-debug-expand'
        className='flex items-center gap-2 border-border border-t bg-card px-3 py-1.5 text-left text-muted-foreground text-xs transition-colors hover:text-foreground'
      >
        <Activity className='h-3.5 w-3.5' />
        <span className='font-medium'>Network</span>
        <span className='bg-muted px-1.5 font-mono text-[11px]'>{entries.length}</span>
        <ChevronUp className='ml-auto h-3.5 w-3.5' />
      </button>
    );
  }

  return (
    <div
      className='flex h-80 flex-col border-border border-t bg-card'
      data-testid='admin-app-debug'
    >
      <div className='flex items-center gap-2 border-border/60 border-b px-3 py-1.5'>
        <Activity className='h-3.5 w-3.5 text-muted-foreground' />
        <span className='font-medium text-xs'>Network</span>
        <span className='bg-muted px-1.5 font-mono text-[11px] text-muted-foreground'>
          {entries.length}
        </span>
        <div className='ml-auto flex items-center gap-1'>
          <Button
            variant='ghost'
            size='sm'
            className='h-6 gap-1 px-2 text-muted-foreground text-xs'
            disabled={entries.length === 0}
            onClick={handleClear}
            data-testid='admin-app-debug-clear'
          >
            <Trash2 className='h-3.5 w-3.5' />
            Clear
          </Button>
          <Button
            variant='ghost'
            size='icon'
            className='h-6 w-6 text-muted-foreground'
            onClick={onToggle}
            aria-label='Collapse network panel'
            data-testid='admin-app-debug-collapse'
          >
            <ChevronDown className='h-3.5 w-3.5' />
          </Button>
        </div>
      </div>

      {entries.length === 0 ? (
        <div className='flex flex-1 items-center justify-center px-4 text-center text-muted-foreground text-xs'>
          No requests yet. Interact with the app — its calls to{" "}
          <span className='font-mono'>/api/*</span> and its functions will appear here.
        </div>
      ) : (
        <div className='flex min-h-0 flex-1'>
          <ul
            className={cn(
              "min-h-0 overflow-auto",
              selected ? "w-2/5 border-border/60 border-r" : "w-full"
            )}
          >
            {entries.map((e) => (
              <li key={e.id}>
                <RequestRow
                  entry={e}
                  selected={e.id === selectedId}
                  compact={selected != null}
                  onSelect={() => setSelectedId(e.id)}
                />
              </li>
            ))}
          </ul>
          {selected && (
            <RequestDetail
              entry={selected}
              appId={appId}
              onClose={() => setSelectedId(null)}
              className='min-h-0 flex-1'
            />
          )}
        </div>
      )}
    </div>
  );
};
