import { Building2, ChevronsUpDown, Search } from "lucide-react";
import { useMemo, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { cn } from "@/libs/shadcn/utils";
import type { AdminOrgMeta } from "@/services/api/adminTenants";

/** Most-recently-opened tenants, newest first. Capped so the list stays scannable. */
const RECENTS_KEY = "oxy.admin.tenant.recents";
const RECENTS_MAX = 5;

function readRecents(): string[] {
  try {
    const raw = localStorage.getItem(RECENTS_KEY);
    return raw ? (JSON.parse(raw) as string[]).slice(0, RECENTS_MAX) : [];
  } catch {
    // A corrupt or unavailable store must not break the header. Recents are a
    // convenience; the switcher below still lists every org.
    return [];
  }
}

export function pushRecent(orgId: string) {
  try {
    const next = [orgId, ...readRecents().filter((id) => id !== orgId)].slice(0, RECENTS_MAX);
    localStorage.setItem(RECENTS_KEY, JSON.stringify(next));
  } catch {
    /* see readRecents */
  }
}

/**
 * Jump between tenants without going back through the directory.
 *
 * Recents first, then everything — because the operator work that needs a switcher is
 * usually moving between the two or three tenants involved in one incident, and a
 * flat alphabetical list makes that the slowest possible path.
 */
export function TenantSwitcher({
  orgs,
  selected,
  onSelect
}: {
  orgs: AdminOrgMeta[];
  selected?: AdminOrgMeta;
  onSelect: (orgId: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState("");

  const recentIds = readRecents();
  const { recents, rest } = useMemo(() => {
    const needle = search.trim().toLowerCase();
    const match = (o: AdminOrgMeta) =>
      !needle || o.name.toLowerCase().includes(needle) || o.slug.toLowerCase().includes(needle);
    const filtered = orgs.filter(match);
    // Recents keep their recency order, not the list's — that ordering IS the feature.
    const recents = recentIds
      .map((id) => filtered.find((o) => o.id === id))
      .filter((o): o is AdminOrgMeta => !!o && o.id !== selected?.id);
    const recentSet = new Set(recents.map((o) => o.id));
    return { recents, rest: filtered.filter((o) => !recentSet.has(o.id)) };
  }, [orgs, recentIds, search, selected?.id]);

  const pick = (orgId: string) => {
    pushRecent(orgId);
    onSelect(orgId);
    setOpen(false);
    setSearch("");
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant='ghost'
          size='sm'
          className='h-7 gap-1.5 px-2'
          data-testid='admin-tenant-switcher'
        >
          <Building2 className='size-3.5 text-muted-foreground' />
          <span className='max-w-40 truncate font-medium text-xs'>
            {selected ? selected.name : "Select tenant"}
          </span>
          <ChevronsUpDown className='size-3 text-muted-foreground' />
        </Button>
      </PopoverTrigger>

      <PopoverContent align='start' className='w-72 p-0'>
        <div className='relative border-b'>
          <Search className='absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground' />
          <Input
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder='Search tenants'
            className='h-9 border-0 pl-7 text-xs focus-visible:ring-0'
            data-testid='admin-tenant-switcher-search'
          />
        </div>
        <div className='max-h-72 overflow-auto py-1'>
          {recents.length > 0 && (
            <Section label='Recent'>
              {recents.map((o) => (
                <Row key={o.id} org={o} onPick={pick} />
              ))}
            </Section>
          )}
          {rest.length > 0 && (
            <Section label={recents.length > 0 ? "All tenants" : undefined}>
              {rest.map((o) => (
                <Row key={o.id} org={o} onPick={pick} selected={o.id === selected?.id} />
              ))}
            </Section>
          )}
          {recents.length === 0 && rest.length === 0 && (
            <p className='px-3 py-4 text-center text-muted-foreground text-xs'>No tenants match.</p>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}

function Section({ label, children }: { label?: string; children: React.ReactNode }) {
  return (
    <div>
      {label && (
        <p className='px-3 pt-2 pb-1 text-[10px] text-muted-foreground uppercase tracking-[0.16em]'>
          {label}
        </p>
      )}
      {children}
    </div>
  );
}

function Row({
  org,
  onPick,
  selected
}: {
  org: AdminOrgMeta;
  onPick: (id: string) => void;
  selected?: boolean;
}) {
  return (
    <button
      type='button'
      onClick={() => onPick(org.id)}
      className={cn(
        "flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs hover:bg-muted/50",
        selected && "bg-muted"
      )}
    >
      <span className='truncate font-medium'>{org.name}</span>
      <span className='truncate font-mono text-[10px] text-muted-foreground'>{org.slug}</span>
    </button>
  );
}
