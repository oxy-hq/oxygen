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
import { useAdminOrgsList } from "@/hooks/api/adminTenants/useAdminOrgs";
import { cn } from "@/libs/shadcn/utils";

/** Wait for typing to settle before it reaches a query key. */
function useDebounced<T>(value: T, delayMs: number): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setSettled(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);
  return settled;
}

const SEARCH_DEBOUNCE_MS = 250;

/**
 * An org, chosen from the server-searched admin directory — the same list and the
 * same Popover + Command shape as Airway's workspace picker, since the directory is
 * cross-tenant and too large to filter in the browser.
 */
export function OrgPicker({
  value,
  onChange
}: {
  /** The chosen org's name, once known; `null` when none is chosen. */
  value: { id: string; name: string | null } | null;
  onChange: (org: { id: string; name: string }) => void;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const search = useDebounced(query, SEARCH_DEBOUNCE_MS);
  const orgs = useAdminOrgsList({ search: search || undefined }, { enabled: open });
  const results = orgs.data ?? [];

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          type='button'
          variant='outline'
          size='sm'
          role='combobox'
          aria-expanded={open}
          className='w-64 justify-between font-normal'
          data-testid='admin-data-flow-org-picker'
        >
          <span className={cn("truncate", !value && "text-muted-foreground")}>
            {value ? (value.name ?? "Loading…") : "Choose an organization…"}
          </span>
          <ChevronsUpDown className='ml-2 size-3 shrink-0 opacity-50' />
        </Button>
      </PopoverTrigger>
      <PopoverContent className={`${DESIGN_REFERENCE} w-72 p-0`} align='start'>
        <Command shouldFilter={false}>
          <CommandInput
            placeholder='Search organizations…'
            value={query}
            onValueChange={setQuery}
          />
          <CommandList>
            {orgs.isFetching && results.length === 0 ? (
              <div className='flex items-center justify-center gap-2 py-6 text-muted-foreground text-xs'>
                <Spinner className='size-3.5' /> Searching…
              </div>
            ) : (
              <>
                <CommandEmpty>
                  {orgs.isError ? "Could not load organizations." : "No organizations found."}
                </CommandEmpty>
                <CommandGroup>
                  {results.map((org) => (
                    <CommandItem
                      key={org.id}
                      value={org.id}
                      onSelect={() => {
                        onChange({ id: org.id, name: org.name });
                        setOpen(false);
                      }}
                    >
                      <span className='flex-1 truncate'>{org.name}</span>
                      <span className='font-mono text-[11px] text-muted-foreground'>
                        /{org.slug}
                      </span>
                      <Check
                        className={cn(
                          "ml-2 size-3",
                          value?.id === org.id ? "opacity-100" : "opacity-0"
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
