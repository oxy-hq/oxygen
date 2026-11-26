import { Check, ChevronsUpDown } from "lucide-react";
import { useEffect, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Button } from "@/components/ui/shadcn/button";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList
} from "@/components/ui/shadcn/command";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useAdminWorkspacesList } from "@/hooks/api/adminTenants/useAdminWorkspaces";
import { cn } from "@/libs/shadcn/utils";

interface WorkspacePickerProps {
  value: { id: string; name: string } | null;
  onChange: (workspace: { id: string; name: string }) => void;
  /** Already-overridden workspaces for this kind — hidden from the results
   *  so picking one can't be mistaken for editing an existing override;
   *  removing the old one first makes the replace explicit. */
  excludeIds: string[];
}

/**
 * A workspace, chosen from a live, cross-tenant, server-searched list — not
 * typed as a raw UUID. Reuses `useAdminWorkspacesList` (the same admin
 * workspace directory `AdminEntitySearch`'s ⌘K palette and `AdminWorkspaces`
 * list page already read from) rather than adding a new endpoint.
 *
 * Built on the same `Popover` + `Command` primitives as the shared
 * `Combobox`, not `Combobox` itself: `Combobox` filters a fixed `items`
 * array client-side, but the workspace directory is cross-tenant and
 * potentially large, so this searches server-side via the hook's `search`
 * param (mirroring `AdminEntitySearch`'s precedent) and lets `Command`'s
 * built-in filter do a final pass on the returned page.
 */
/**
 * Wait for typing to settle before letting a value reach a query key.
 *
 * Local and tiny on purpose — one caller, so it lives beside it. Colocated
 * rather than in `hooks/`, per this app's recursive-colocation rule; promote it
 * the day a second picker needs it.
 */
function useDebounced<T>(value: T, delayMs: number): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setSettled(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);
  return settled;
}

/** Long enough to swallow a burst of typing, short enough not to feel laggy. */
const SEARCH_DEBOUNCE_MS = 250;

export function WorkspacePicker({ value, onChange, excludeIds }: WorkspacePickerProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  // The input stays instant; only the *request* waits. Without this every
  // keystroke was a fresh cross-tenant workspace search.
  const debouncedQuery = useDebounced(query, SEARCH_DEBOUNCE_MS);

  const workspaces = useAdminWorkspacesList(
    { search: debouncedQuery },
    // Previous results stay on screen while the next search is in flight —
    // a changed term is a new query key, so without this the list emptied and
    // the whole popover flipped to "Searching…" between every keystroke.
    { enabled: open, keepPreviousData: true }
  );
  const results = (workspaces.data ?? []).filter((ws) => !excludeIds.includes(ws.id));
  // Only when there is genuinely nothing to show. `isFetching` alone replaced a
  // perfectly good list with a spinner on every background refetch.
  const showSpinner = workspaces.isFetching && results.length === 0;

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type='button'
          variant='outline'
          role='combobox'
          aria-expanded={open}
          className='w-full justify-between bg-input/30 font-normal'
          data-testid='admin-airway-override-workspace-picker'
        >
          <span className={cn("truncate", !value && "text-muted-foreground")}>
            {value ? value.name : "Select a workspace…"}
          </span>
          <ChevronsUpDown className='ml-2 size-4 shrink-0 opacity-50' />
        </Button>
      </PopoverTrigger>
      <PopoverContent className={`${DESIGN_REFERENCE} w-(--radix-popover-trigger-width) p-0`}>
        <Command shouldFilter={false}>
          <CommandInput placeholder='Search workspaces…' value={query} onValueChange={setQuery} />
          <CommandList>
            {showSpinner ? (
              <div className='flex items-center justify-center gap-2 py-6 text-muted-foreground text-xs'>
                <Spinner className='size-3.5' /> Searching…
              </div>
            ) : (
              <>
                <CommandEmpty>No workspaces found.</CommandEmpty>
                <CommandGroup>
                  {results.map((ws) => (
                    <CommandItem
                      key={ws.id}
                      value={ws.id}
                      onSelect={() => {
                        onChange({ id: ws.id, name: ws.name });
                        setOpen(false);
                      }}
                    >
                      <span className='flex-1 truncate'>{ws.name}</span>
                      {ws.org_slug && (
                        <span className='font-mono text-[11px] text-muted-foreground'>
                          /{ws.org_slug}
                        </span>
                      )}
                      <Check
                        className={cn(
                          "ml-2 size-4",
                          value?.id === ws.id ? "opacity-100" : "opacity-0"
                        )}
                      />
                    </CommandItem>
                  ))}
                </CommandGroup>
              </>
            )}
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
