import { Check, ChevronDown, Globe, MapPin } from "lucide-react";
import type React from "react";
import { useMemo } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger
} from "@/components/ui/shadcn/dropdown-menu";
import useSites from "@/hooks/api/cameras/useSites";
import { useSelectedSite } from "@/hooks/useSelectedSite";
import { cn } from "@/libs/shadcn/utils";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * Global site selector mounted in the Edge layout header. Persists
 * to URL via `useSelectedSite` so deep-links honor the operator's
 * chosen scope and switching tabs preserves the filter.
 *
 * "All sites" (siteId === undefined) is the default — sentinel for
 * "no per-site filter."
 */
const SitePicker: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  const { data: sites = [] } = useSites(workspace?.id);
  const { siteId, setSiteId } = useSelectedSite();

  // Alphabetical for predictability; same ordering whether the
  // operator's looking at the picker or the topology graph.
  const sortedSites = useMemo(
    () => [...sites].sort((a, b) => a.name.localeCompare(b.name)),
    [sites]
  );

  const current = sortedSites.find((s) => s.id === siteId);
  const label = current ? current.name : "All sites";
  const Icon = current ? MapPin : Globe;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        className={cn(
          "inline-flex items-center gap-1.5 border bg-card px-2.5 py-1 font-medium text-sm outline-none transition-colors",
          "hover:bg-muted",
          current && "border-primary/40 bg-primary/5"
        )}
        data-testid='site-picker'
        // No sites? Still render so the operator can see the empty
        // state — the dropdown will say "No sites yet" instead of
        // being disabled silently.
      >
        <Icon className='h-3.5 w-3.5 text-muted-foreground' />
        <span className='max-w-40 truncate'>{label}</span>
        <ChevronDown className='h-3 w-3 text-muted-foreground' />
      </DropdownMenuTrigger>
      <DropdownMenuContent align='start' className={`${DESIGN_REFERENCE} min-w-56`}>
        <DropdownMenuItem
          onClick={() => setSiteId(undefined)}
          className='cursor-pointer'
          data-testid='site-picker-all'
        >
          <Globe className='h-3.5 w-3.5 text-muted-foreground' />
          <span className='flex-1'>All sites</span>
          {siteId === undefined && <Check className='h-3.5 w-3.5' />}
        </DropdownMenuItem>
        {sortedSites.length > 0 && <DropdownMenuSeparator />}
        {sortedSites.length === 0 ? (
          <div className='px-2 py-1.5 text-muted-foreground text-xs'>
            No sites yet — add one from Devices.
          </div>
        ) : (
          sortedSites.map((s) => (
            <DropdownMenuItem
              key={s.id}
              onClick={() => setSiteId(s.id)}
              className='cursor-pointer'
              data-testid={`site-picker-${s.id}`}
            >
              <MapPin className='h-3.5 w-3.5 text-muted-foreground' />
              <div className='flex min-w-0 flex-1 flex-col'>
                <span className='truncate'>{s.name}</span>
                {s.region && (
                  <span className='truncate text-muted-foreground text-xs'>{s.region}</span>
                )}
              </div>
              {siteId === s.id && <Check className='h-3.5 w-3.5' />}
            </DropdownMenuItem>
          ))
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
};

export default SitePicker;
