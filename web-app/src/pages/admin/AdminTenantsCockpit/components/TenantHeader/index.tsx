import { FolderTree, List } from "lucide-react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { cn } from "@/libs/shadcn/utils";
import { SWITCHER_TYPES, type TenantType, type TenantView } from "../../useTenantSelection";
import CreateOrgDialog from "../CreateOrgDialog";
import { TenantActivity } from "./components/TenantActivity";
import { TenantSummary } from "./components/TenantSummary";
import { TenantSwitcher } from "./components/TenantSwitcher";
import { useSelectedTenant } from "./useSelectedTenant";

/**
 * The tenant surface header. Two modes, because the useful header is a different one
 * depending on whether a tenant is open:
 *
 * * **Browsing** (no selection, or a non-org entity) — the entity switcher and the map
 *   toggle, as before. Nothing to summarise yet.
 * * **A tenant is open** — it becomes that tenant's header: a switcher to jump straight
 *   to another, its counts inline, and who last touched it.
 *
 * **It carries no actions.** An earlier cut added "Act as" here — which the dossier
 * header already offers, forty pixels below, alongside Edit / Delete / Managed-by. Two
 * buttons for one action in one viewport is not a more powerful header, it is a louder
 * one. What belongs up here is what the dossier structurally cannot do: leave (the
 * switcher), and show history (activity).
 *
 * Everything editable stays in the dossier below. An action with two homes is an action
 * whose two homes eventually disagree; the header's job is the things the dossier can't
 * do — leaving, comparing, and entering.
 */
export default function TenantHeader({
  type,
  id,
  onTypeChange,
  onSelect,
  view,
  onViewChange,
  onCreatedOrg
}: {
  type: TenantType;
  /** The selected entity, or null while browsing. */
  id: string | null;
  onTypeChange: (t: TenantType) => void;
  /** Select a tenant (used by the switcher). */
  onSelect: (id: string) => void;
  view: TenantView;
  onViewChange: (v: TenantView) => void;
  /** Called with the new org id after a successful create, so the cockpit can select it. */
  onCreatedOrg?: (orgId: string) => void;
}) {
  const { isOrg, org, orgs, activity, activityPending, activityError } = useSelectedTenant(
    type,
    id
  );

  // Provisioning a tenant only makes sense on the org list — the map and the other
  // entity types have nothing to create here.
  const canCreateOrg = type === "orgs" && view === "list" && !isOrg;

  return (
    // On a phone the bar wraps instead of dropping anything: the switcher and the controls
    // keep the first line, the counts take a line of their own (`max-sm:contents` lifts
    // the left group's children into the bar so the counts can be ordered last).
    <header className='flex h-11 shrink-0 items-center justify-between gap-3 border-b px-3 max-sm:h-auto max-sm:min-h-11 max-sm:flex-wrap max-sm:gap-y-1 max-sm:py-1.5'>
      <div className='flex min-w-0 items-center gap-3 max-sm:contents'>
        {isOrg && org ? (
          <>
            <TenantSwitcher orgs={orgs} selected={org} onSelect={onSelect} />
            <div className='flex min-w-0 items-center gap-3 max-sm:order-last max-sm:basis-full'>
              <span className='h-4 w-px shrink-0 bg-border max-sm:hidden' />
              <TenantSummary org={org} />
            </div>
          </>
        ) : (
          <>
            <span className='font-semibold text-xs'>Tenants</span>
            {/* The switcher only makes sense in list view. The map spans all three
                entities at once, so switching type there changes nothing — hide it
                rather than offer a no-op control. */}
            {view === "list" ? (
              <div className='flex items-center gap-0.5 rounded-md bg-muted/60 p-0.5'>
                {SWITCHER_TYPES.map((t) => (
                  <button
                    key={t.id}
                    type='button'
                    data-testid={`tenant-type-${t.id}`}
                    onClick={() => onTypeChange(t.id)}
                    className={cn(
                      "rounded px-2.5 py-1 font-medium text-xs transition-colors",
                      type === t.id
                        ? "bg-background text-foreground shadow-sm"
                        : "text-muted-foreground hover:text-foreground"
                    )}
                  >
                    <span className='max-sm:hidden'>{t.label}</span>
                    <span className='sm:hidden'>{t.short}</span>
                  </button>
                ))}
              </div>
            ) : (
              <span className='text-muted-foreground text-xs'>Relationship map</span>
            )}
          </>
        )}
      </div>

      <div className='flex shrink-0 items-center gap-2'>
        {isOrg && org && (
          <div className='flex items-center gap-2'>
            <TenantActivity events={activity} isPending={activityPending} isError={activityError} />
            <span className='h-4 w-px bg-border' />
          </div>
        )}
        {canCreateOrg && <CreateOrgDialog onCreated={onCreatedOrg} />}
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type='button'
              data-testid={view === "map" ? "tenant-view-list" : "tenant-view-map"}
              onClick={() => onViewChange(view === "map" ? "list" : "map")}
              className={cn(
                "flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground",
                view === "map" && "bg-muted text-foreground"
              )}
            >
              {view === "map" ? <List className='size-4' /> : <FolderTree className='size-4' />}
            </button>
          </TooltipTrigger>
          <TooltipContent>{view === "map" ? "Back to list" : "Relationship map"}</TooltipContent>
        </Tooltip>
      </div>
    </header>
  );
}
