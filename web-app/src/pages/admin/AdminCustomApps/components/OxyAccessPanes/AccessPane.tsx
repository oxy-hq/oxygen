import { Building2, LayoutGrid, List, Lock } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import { useAllAdminOrgs } from "@/hooks/api/adminTenants/useAdminOrgs";
import { useAdminApps } from "@/hooks/api/customApps/useCustomApps";
import { useOxyAccessGrants } from "@/hooks/api/customApps/useOxyAccessGrants";
import { cn } from "@/libs/shadcn/utils";
import type { CustomApp } from "@/types/apps";
import { type AccessOrg, buildAccessOrgs } from "./accessModel";
import { OrgAccessDetail } from "./OrgAccessDetail";
import {
  AccessStrip,
  EmptyHint,
  GrantsError,
  GrantsLoading,
  ListRow,
  MasterDetail,
  PaneSearch
} from "./shared";

/** Which slice of the org directory the list shows. */
type AccessFilter = "locked" | "open" | "all";

/** List = dense master/detail. Gallery = card grid of orgs. */
type AccessView = "list" | "gallery";

/**
 * Oxy-access browser — every org on the deployment with its workspaces' lockdown
 * state.
 *
 * Inverted 2026-07-14: staff reach every workspace BY DEFAULT, so the question is
 * no longer "who opted in?" but "who locked us OUT?". The list filters by
 * Locked out / Open / All, and locked orgs sort first — they're the exception an
 * operator is hunting when an app won't open.
 *
 * Two views: a dense master/detail **List**, and a **Gallery** of org cards for
 * scanning the fleet at a glance.
 *
 * All orgs come from the admin directory (`/admin/orgs-meta`, same owner/
 * app-admin gate as this tab); lockdown rows from the access endpoint. Both load
 * up front and join client-side — admin scale is dozens to low hundreds.
 */
export const AccessPane = () => {
  const grantsQuery = useOxyAccessGrants();
  // The `?? []` is gated, not a swallow: the failure returns at `GrantsError`
  // below before anything reads `grants`. It is here only because the memos
  // that derive from it run above that gate, as hooks must.
  const grants = useMemo(() => grantsQuery.data ?? [], [grantsQuery.data]);
  const {
    data: orgPages,
    isLoading: orgsLoading,
    hasNextPage,
    isFetchingNextPage,
    fetchNextPage
  } = useAllAdminOrgs();
  // Drain every page so the org directory (and the "no access" set + strip
  // counts) is exhaustive, not capped at the first server page.
  useEffect(() => {
    if (hasNextPage && !isFetchingNextPage) fetchNextPage();
  }, [hasNextPage, isFetchingNextPage, fetchNextPage]);
  const adminOrgs = useMemo(() => orgPages?.pages.flat() ?? [], [orgPages]);
  const accessOrgs = useMemo(() => buildAccessOrgs(adminOrgs, grants), [adminOrgs, grants]);

  // Join in each org's custom apps so its detail lists what it owns. Same
  // infinite query the Apps tab uses (shared React Query cache), drained fully.
  const {
    data: appPages,
    hasNextPage: hasMoreApps,
    isFetchingNextPage: fetchingApps,
    fetchNextPage: fetchMoreApps
  } = useAdminApps(100);
  useEffect(() => {
    if (hasMoreApps && !fetchingApps) fetchMoreApps();
  }, [hasMoreApps, fetchingApps, fetchMoreApps]);
  const appsByOrg = useMemo(() => {
    const map = new Map<string, CustomApp[]>();
    for (const a of appPages?.pages.flatMap((p) => p.items) ?? []) {
      const list = map.get(a.org_id);
      if (list) list.push(a);
      else map.set(a.org_id, [a]);
    }
    return map;
  }, [appPages]);

  const [search, setSearch] = useState("");
  const [filter, setFilter] = useState<AccessFilter>("all");
  const [view, setView] = useState<AccessView>("list");
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    return accessOrgs.filter((o) => {
      if (filter === "locked" && !o.hasLockdown) return false;
      if (filter === "open" && o.hasLockdown) return false;
      if (!q) return true;
      if (`${o.orgName} ${o.orgSlug}`.toLowerCase().includes(q)) return true;
      return o.workspaces.some((w) => w.workspace_name.toLowerCase().includes(q));
    });
  }, [accessOrgs, filter, search]);

  const selected = accessOrgs.find((o) => o.orgId === selectedId) ?? filtered[0] ?? null;
  const lockedOrgs = useMemo(() => accessOrgs.filter((o) => o.hasLockdown).length, [accessOrgs]);

  if (grantsQuery.error)
    return <GrantsError error={grantsQuery.error} onRetry={grantsQuery.refetch} />;
  if (grantsQuery.isLoading || orgsLoading) return <GrantsLoading />;
  if (accessOrgs.length === 0) {
    return (
      <EmptyHint
        title='No organizations'
        body='No organizations exist on this deployment yet. When one is created it shows up here.'
      />
    );
  }

  return (
    <div className='flex min-h-0 flex-1 flex-col'>
      <div className='flex items-center gap-2 border-b'>
        <div className='min-w-0 flex-1'>
          <AccessStrip
            orgs={accessOrgs.length}
            withAccess={lockedOrgs}
            workspaces={grants.length}
          />
        </div>
        <div className='mr-3 flex shrink-0 items-center gap-0.5 rounded-md bg-muted/60 p-0.5 pr-1'>
          <ViewButton
            active={view === "list"}
            onClick={() => setView("list")}
            icon={<List className='size-3.5' />}
            label='List'
          />
          <ViewButton
            active={view === "gallery"}
            onClick={() => setView("gallery")}
            icon={<LayoutGrid className='size-3.5' />}
            label='Gallery'
          />
        </div>
      </div>

      {view === "gallery" ? (
        <OrgGallery
          orgs={filtered}
          filter={filter}
          onFilterChange={setFilter}
          search={search}
          onSearchChange={setSearch}
          onOpen={(id) => {
            setSelectedId(id);
            setView("list");
          }}
        />
      ) : (
        <MasterDetail
          list={
            <div className='flex h-full flex-col'>
              <PaneSearch
                value={search}
                onChange={setSearch}
                placeholder='Search orgs or workspaces…'
              />
              <div className='flex items-center justify-between gap-2 border-border/60 border-b px-2 py-1.5'>
                <Select value={filter} onValueChange={(v) => setFilter(v as AccessFilter)}>
                  <SelectTrigger
                    className='h-7 w-auto gap-1 px-2 text-xs'
                    aria-label='Access filter'
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent align='start'>
                    <SelectItem value='all'>All orgs</SelectItem>
                    <SelectItem value='locked'>Locked out</SelectItem>
                    <SelectItem value='open'>Open to Oxy</SelectItem>
                  </SelectContent>
                </Select>
                <span className='font-mono text-[11px] text-muted-foreground tabular-nums'>
                  {filtered.length}
                </span>
              </div>
              <div className='min-h-0 flex-1 overflow-auto'>
                {filtered.length === 0 ? (
                  <p className='px-3 py-6 text-center text-muted-foreground text-xs'>
                    No orgs match this filter.
                  </p>
                ) : (
                  filtered.map((o) => (
                    <ListRow
                      key={o.orgId}
                      active={o.orgId === selected?.orgId}
                      onClick={() => setSelectedId(o.orgId)}
                      title={o.orgName}
                      subtitle={o.orgSlug}
                      trailing={
                        o.hasLockdown ? (
                          <span className='flex items-center gap-1 rounded bg-destructive/10 px-1.5 py-0.5 font-medium text-[10px] text-destructive uppercase tracking-wide'>
                            <Lock className='size-2.5' />
                            {o.lockedCount}
                          </span>
                        ) : (
                          <span className='rounded bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground tabular-nums'>
                            {o.accessibleCount}
                          </span>
                        )
                      }
                    />
                  ))
                )}
              </div>
            </div>
          }
          detail={
            selected ? (
              <OrgAccessDetail
                org={selected}
                apps={appsByOrg.get(selected.orgId) ?? []}
                highlight={search.trim().toLowerCase()}
              />
            ) : null
          }
        />
      )}
    </div>
  );
};

const ViewButton = ({
  active,
  onClick,
  icon,
  label
}: {
  active: boolean;
  onClick: () => void;
  icon: React.ReactNode;
  label: string;
}) => (
  <button
    type='button'
    onClick={onClick}
    className={cn(
      "flex items-center gap-1.5 rounded px-2 py-1 font-medium text-xs transition-colors",
      active
        ? "bg-background text-foreground shadow-sm"
        : "text-muted-foreground hover:text-foreground"
    )}
  >
    {icon}
    {label}
  </button>
);

/**
 * Gallery view — the fleet at a glance. One card per org: how many workspaces are
 * open to Oxy and how many the org has locked us out of. Locked orgs carry a
 * destructive accent so the exceptions pop out of the grid. Clicking a card opens
 * that org in the List view's detail pane.
 */
const OrgGallery = ({
  orgs,
  filter,
  onFilterChange,
  search,
  onSearchChange,
  onOpen
}: {
  orgs: AccessOrg[];
  filter: AccessFilter;
  onFilterChange: (f: AccessFilter) => void;
  search: string;
  onSearchChange: (s: string) => void;
  onOpen: (orgId: string) => void;
}) => (
  <div className='flex min-h-0 flex-1 flex-col'>
    <div className='flex items-center gap-2 border-b px-3 py-2'>
      <div className='w-64'>
        <PaneSearch value={search} onChange={onSearchChange} placeholder='Search orgs…' />
      </div>
      <Select value={filter} onValueChange={(v) => onFilterChange(v as AccessFilter)}>
        <SelectTrigger className='h-7 w-auto gap-1 px-2 text-xs' aria-label='Access filter'>
          <SelectValue />
        </SelectTrigger>
        <SelectContent align='start'>
          <SelectItem value='all'>All orgs</SelectItem>
          <SelectItem value='locked'>Locked out</SelectItem>
          <SelectItem value='open'>Open to Oxy</SelectItem>
        </SelectContent>
      </Select>
      <span className='ml-auto font-mono text-[11px] text-muted-foreground tabular-nums'>
        {orgs.length}
      </span>
    </div>

    <div className='min-h-0 flex-1 overflow-auto p-3'>
      {orgs.length === 0 ? (
        <p className='py-10 text-center text-muted-foreground text-xs'>
          No orgs match this filter.
        </p>
      ) : (
        <div className='grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3'>
          {orgs.map((o) => (
            <button
              key={o.orgId}
              type='button'
              onClick={() => onOpen(o.orgId)}
              className={cn(
                "flex flex-col gap-2 rounded-lg border p-3 text-left transition-colors hover:bg-muted/40",
                o.hasLockdown ? "border-destructive/40 bg-destructive/5" : "border-border/60"
              )}
            >
              <div className='flex items-start gap-2'>
                <div
                  className={cn(
                    "flex size-7 shrink-0 items-center justify-center rounded-md border",
                    o.hasLockdown
                      ? "bg-destructive/10 text-destructive"
                      : "bg-muted/40 text-muted-foreground"
                  )}
                >
                  {o.hasLockdown ? (
                    <Lock className='size-3.5' />
                  ) : (
                    <Building2 className='size-3.5' />
                  )}
                </div>
                <div className='min-w-0 flex-1'>
                  <p className='truncate font-medium text-xs leading-tight'>{o.orgName}</p>
                  <p className='truncate font-mono text-[11px] text-muted-foreground'>
                    {o.orgSlug}
                  </p>
                </div>
              </div>

              <div className='flex items-center gap-1.5 text-[11px]'>
                <span className='rounded bg-muted px-1.5 py-0.5 font-mono text-muted-foreground tabular-nums'>
                  {o.accessibleCount} open
                </span>
                {o.lockedCount > 0 && (
                  <span className='rounded bg-destructive/10 px-1.5 py-0.5 font-mono text-destructive tabular-nums'>
                    {o.lockedCount} locked
                  </span>
                )}
              </div>
            </button>
          ))}
        </div>
      )}
    </div>
  </div>
);
