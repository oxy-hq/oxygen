import { Search } from "lucide-react";
import { useState } from "react";
import { Input } from "@/components/ui/shadcn/input";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { cn } from "@/libs/shadcn/utils";
import type { ChildOrg } from "@/types/partners";
import CreateClientDialog from "../../components/CreateClientDialog";

/**
 * The client list — the partner-scoped mirror of the admin `TenantRail`. Dense,
 * searchable, one click to select. "New client" lives here when the partner may
 * onboard (`create_orgs`).
 */
export default function ClientRail({
  orgs,
  isLoading,
  selectedId,
  onSelect,
  partnerId,
  canCreate
}: {
  orgs: ChildOrg[] | undefined;
  isLoading: boolean;
  selectedId: string | undefined;
  onSelect: (orgId: string) => void;
  partnerId: string;
  canCreate: boolean;
}) {
  const [q, setQ] = useState("");
  const filtered = (orgs ?? []).filter((o) => {
    const needle = q.trim().toLowerCase();
    if (!needle) return true;
    return o.name.toLowerCase().includes(needle) || o.slug.toLowerCase().includes(needle);
  });

  return (
    <div className='flex min-h-0 flex-col'>
      <div className='space-y-2 border-b p-3'>
        <div className='flex items-center justify-between gap-2'>
          <p className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
            Clients
          </p>
          {canCreate && <CreateClientDialog partnerId={partnerId} />}
        </div>
        <div className='relative'>
          <Search className='absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground' />
          <Input
            value={q}
            onChange={(e) => setQ(e.target.value)}
            placeholder='Search clients…'
            className='h-8 pl-7 text-sm'
            data-testid='partner-clients-search'
          />
        </div>
      </div>

      <div className='min-h-0 flex-1 overflow-y-auto p-1.5'>
        {isLoading ? (
          <div className='space-y-1.5 p-1.5'>
            <Skeleton className='h-11 w-full' />
            <Skeleton className='h-11 w-full' />
          </div>
        ) : filtered.length === 0 ? (
          <p className='p-3 text-muted-foreground text-xs'>
            {orgs?.length ? "No clients match." : "No clients yet."}
          </p>
        ) : (
          filtered.map((o) => (
            <button
              key={o.org_id}
              type='button'
              onClick={() => onSelect(o.org_id)}
              aria-pressed={o.org_id === selectedId}
              data-testid={`partner-client-${o.slug}`}
              className={cn(
                "w-full px-2.5 py-2 text-left transition-colors",
                o.org_id === selectedId ? "bg-muted" : "hover:bg-muted/50"
              )}
            >
              <div className='truncate font-medium text-sm'>{o.name}</div>
              <div className='flex items-center gap-2 text-muted-foreground text-xs'>
                <span className='truncate'>{o.slug}</span>
                <span className='shrink-0 tabular-nums'>
                  {o.member_count} member{o.member_count === 1 ? "" : "s"} · {o.app_count} app
                  {o.app_count === 1 ? "" : "s"}
                </span>
              </div>
            </button>
          ))
        )}
      </div>
    </div>
  );
}
