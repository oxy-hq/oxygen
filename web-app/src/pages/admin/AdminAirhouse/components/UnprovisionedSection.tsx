import { ChevronRight } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import { AdminSectionLabel } from "@/pages/admin/components/AdminSectionLabel";
import type { AirhouseFleetRow as Row } from "@/services/api/airhouseAdmin";
import { AirhouseUnprovisionedRow } from "./AirhouseUnprovisionedRow";

/**
 * Workspaces with no warehouse, behind a disclosure.
 *
 * On a real deployment this half is most of the fleet and none of the incident,
 * and as an always-open grid it pushed the rows an operator came for off the
 * screen. It costs one line until asked for — but only when collapsing earns
 * something. Whether it starts open is the caller's decision, not this
 * component's: see `showUnprovisioned` in the page, which keeps a short list —
 * or one the operator has already narrowed — open rather than charging a click
 * to reveal four rows.
 *
 * The count sits on the label whether open or closed, so a collapsed section is
 * never silent about how much it is holding.
 */
export const UnprovisionedSection = ({
  rows,
  open,
  onToggle
}: {
  rows: Row[];
  open: boolean;
  onToggle: () => void;
}) => (
  <div className='flex flex-col gap-2' data-testid='admin-airhouse-unprovisioned'>
    <button
      type='button'
      onClick={onToggle}
      className='-mx-1 flex items-center gap-1 px-1 py-0.5 text-left outline-none hover:bg-accent/60 focus-visible:ring-2 focus-visible:ring-ring/50'
      aria-expanded={open}
      data-testid='admin-airhouse-unprovisioned-toggle'
    >
      <ChevronRight
        className={cn(
          "size-3 shrink-0 text-muted-foreground transition-transform",
          open && "rotate-90"
        )}
      />
      <AdminSectionLabel trailing={String(rows.length)}>No warehouse</AdminSectionLabel>
    </button>
    {open && (
      <div className='grid grid-cols-1 gap-x-6 md:grid-cols-2 xl:grid-cols-3'>
        {rows.map((r) => (
          <AirhouseUnprovisionedRow key={r.workspace_id} row={r} />
        ))}
      </div>
    )}
  </div>
);
