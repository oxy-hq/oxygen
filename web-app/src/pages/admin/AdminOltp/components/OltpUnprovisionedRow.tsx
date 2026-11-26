import { Database, Loader2 } from "lucide-react";
import { useProvisionOltp } from "@/hooks/api/oltp/useAdminOltp";
import type { OltpTenantRow } from "@/services/api/oltp";

/**
 * An org with no database: a name and the one action that applies.
 *
 * Deliberately not the same row as a provisioned tenant. Rendering these
 * through the shared table meant three columns of `—` repeated for every org
 * that had not been provisioned yet — on a fresh deployment that is nearly the
 * whole page, and it made a list of *absences* as visually heavy as the list of
 * real databases.
 *
 * A plain `<button>`, not `<Button>`: the shared button carries `.t-button`,
 * which sets font-size and weight from CSS variables in an unlayered rule, so a
 * `text-[11px]` utility on the element does not win and the label renders at
 * 14px/500 — heavier than the org name it belongs to. Several admin call sites
 * pass `text-[11px]` to `<Button>` today and silently get 14px. Micro-actions
 * in this surface are plain buttons; `<Button>` stays for the real ones.
 */
export const OltpUnprovisionedRow = ({ row }: { row: OltpTenantRow }) => {
  const provision = useProvisionOltp(row.org_id);
  return (
    <div
      className='flex items-center justify-between gap-2 border-border/60 border-b px-1 py-1 last:border-b-0'
      data-testid={`admin-oltp-row-${row.org_id}`}
    >
      {/* The name leads. A default-weight button label beside muted text put
          the emphasis on the verb rather than on which org it applies to,
          which is the wrong way round when the verb is identical on every row. */}
      <span className='min-w-0 truncate text-xs'>{row.org_name}</span>
      <button
        type='button'
        disabled={provision.isPending}
        onClick={() => provision.mutate([])}
        className='inline-flex shrink-0 items-center gap-1 px-1.5 py-0.5 text-muted-foreground text-xs outline-none transition-colors hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50 disabled:opacity-50'
        data-testid={`admin-oltp-provision-${row.org_id}`}
      >
        {provision.isPending ? (
          <Loader2 className='size-3 animate-spin' />
        ) : (
          <Database className='size-3' />
        )}
        Provision
      </button>
    </div>
  );
};
