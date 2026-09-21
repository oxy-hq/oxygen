import { Search, X } from "lucide-react";
import { useMemo } from "react";
import { useSearchParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useOltpTenants } from "@/hooks/api/oltp/useAdminOltp";
import { cn } from "@/libs/shadcn/utils";
import { AdminAsync } from "../components/AdminAsync";
import { AdminPage } from "../components/AdminPage";
import { AdminSectionLabel } from "../components/AdminSectionLabel";
import { OltpTenantPanel } from "../components/OltpTenantPanel";
import { OltpFleetRow } from "./components/OltpFleetRow";
import { OltpUnprovisionedRow } from "./components/OltpUnprovisionedRow";

/**
 * The OLTP fleet: who has a database, what is inside it, and who still needs one.
 *
 * **Master–detail.** Selecting a tenant opens its management panel beside the
 * list — the same panel the org page shows, so there is one implementation of
 * "manage this database" and two ways in. Before this the row was a link to the
 * org page's OLTP *section*, below Branding, Identity and Members, which sent an
 * operator out of the surface they came to use and left two thirds of this page
 * empty.
 *
 * Selection lives in `?org=`, so a tenant is linkable and Back closes it.
 *
 * **Split by state rather than sorted by it.** One table with a `status` column
 * gave a row of `— — —` the same weight as a real tenant, so on a deployment
 * where six of seven orgs are unprovisioned the page was mostly dashes.
 * Provisioned tenants get a detail row; the rest collapse into a narrow list
 * whose only content is the name and the one action that applies.
 *
 * **No stat tiles.** Four cards showed `1/7`, `2`, `0` and `6` — numbers
 * derivable from the table 40px below, one of them literally
 * `total − provisioned` — in the most valuable band on the page. They are one
 * line in the header now.
 */
const AdminOltp = () => {
  // The whole query: a fleet that failed to load and a deployment with no
  // tenants are different answers, and only `AdminAsync` keeps them apart.
  const tenants = useOltpTenants();
  const { data } = tenants;
  const [params, setParams] = useSearchParams();
  const query = params.get("q") ?? "";
  const selectedOrg = params.get("org");

  const setParam = (k: string, v: string | null) => {
    const next = new URLSearchParams(params);
    if (v) next.set(k, v);
    else next.delete(k);
    setParams(next, { replace: true });
  };

  const { live, empty, summary } = useMemo(() => {
    const all = data ?? [];
    const q = query.trim().toLowerCase();
    const match = (r: (typeof all)[number]) =>
      !q ||
      r.org_name.toLowerCase().includes(q) ||
      r.database.toLowerCase().includes(q) ||
      r.provider.toLowerCase().includes(q) ||
      r.schemas.some((s) => s.schema.toLowerCase().includes(q));

    const provisioned = all.filter((r) => r.status !== "none");
    // Only tenants meant to be serving. A `pending_delete` row is on its way
    // out, so counting its drift inflates a number an operator chases to zero.
    const attention = provisioned.filter(
      (r) => r.status === "active" && (!r.analyst_ready || r.platform_drift)
    ).length;

    return {
      live: provisioned.filter(match),
      // `&& match` — the function, not the call — read as permanently true, so typing in
      // the search box filtered the provisioned list and left the unprovisioned one
      // whole. It typechecks because `filter`'s predicate return is only required to be
      // truthy, so nothing caught it.
      empty: all.filter((r) => r.status === "none" && match(r)),
      summary: {
        provisioned: provisioned.length,
        total: all.length,
        schemaCount: provisioned.reduce((n, r) => n + r.schemas.length, 0),
        attention
      }
    };
  }, [data, query]);

  // Hoisted above the render gate so the page's width can depend on it: with a
  // tenant open the detail panel needs the room, and the list narrows to
  // compensate rather than the page growing a scrollbar. Undefined data means
  // no selection, which is the same `wide` the unselected list wants anyway.
  const selectedRow = (data ?? []).find((r) => r.org_id === selectedOrg);

  return (
    <AdminPage
      width={selectedRow ? "full" : "wide"}
      // Only once there is a fleet to summarise — "0 of 0 organizations
      // provisioned · all healthy" is a false all-clear, not a placeholder.
      description={
        data ? (
          <>
            {summary.provisioned} of {summary.total} organizations provisioned ·{" "}
            {summary.schemaCount} schema{summary.schemaCount === 1 ? "" : "s"} ·{" "}
            {summary.attention === 0 ? (
              "all healthy"
            ) : (
              <span className='text-destructive'>
                {summary.attention} need{summary.attention === 1 ? "s" : ""} attention
              </span>
            )}
          </>
        ) : undefined
      }
      actions={
        <div className='relative w-64'>
          <Search className='absolute top-1/2 left-2 size-3 -translate-y-1/2 text-muted-foreground' />
          <Input
            className='h-7 pl-7 text-xs'
            placeholder='Org, database, provider or schema…'
            value={query}
            onChange={(e) => setParam("q", e.target.value || null)}
            data-testid='admin-oltp-filter'
          />
        </div>
      }
      data-testid='admin-oltp'
    >
      <AdminAsync
        query={tenants}
        noun='the OLTP fleet'
        // A master–detail page, not a row list: one block reads as the split
        // pane that is about to arrive.
        skeleton={<Skeleton className='h-64 w-full' />}
      >
        {() => (
          <div className={cn("grid gap-6", selectedRow && "lg:grid-cols-[22rem_minmax(0,1fr)]")}>
            <div className='flex min-w-0 flex-col gap-4'>
              <div className='flex flex-col gap-2' data-testid='admin-oltp-provisioned'>
                <AdminSectionLabel trailing={String(live.length)}>Provisioned</AdminSectionLabel>
                {live.length === 0 ? (
                  <p
                    className='px-1 py-2 text-muted-foreground text-xs'
                    data-testid='admin-oltp-provisioned-empty'
                  >
                    {query
                      ? "No database matches that filter."
                      : "No organization has a database yet."}
                  </p>
                ) : (
                  <div className='flex flex-col'>
                    {live.map((r) => (
                      <OltpFleetRow
                        key={r.org_id}
                        row={r}
                        compact={Boolean(selectedRow)}
                        selected={r.org_id === selectedOrg}
                        onSelect={() => setParam("org", r.org_id === selectedOrg ? null : r.org_id)}
                      />
                    ))}
                  </div>
                )}
              </div>

              {empty.length > 0 && (
                <div className='flex flex-col gap-2' data-testid='admin-oltp-unprovisioned'>
                  <AdminSectionLabel trailing={String(empty.length)}>No database</AdminSectionLabel>
                  {/* Two columns when nothing is selected: these rows hold one
                      short name each, so one full-width column would leave most
                      of the row empty and double the height of the section an
                      operator cares least about. */}
                  <div className={cn("grid gap-x-6", !selectedRow && "md:grid-cols-2")}>
                    {empty.map((r) => (
                      <OltpUnprovisionedRow key={r.org_id} row={r} />
                    ))}
                  </div>
                </div>
              )}
            </div>

            {selectedRow && (
              <div className='flex min-w-0 flex-col gap-2' data-testid='admin-oltp-detail'>
                <div className='flex items-center justify-between gap-2 border-border/60 border-b pb-2'>
                  <h2 className='truncate font-semibold text-sm'>{selectedRow.org_name}</h2>
                  <Button
                    size='sm'
                    variant='ghost'
                    className='h-6 px-2'
                    onClick={() => setParam("org", null)}
                    data-testid='admin-oltp-detail-close'
                  >
                    <X className='size-3' />
                    Close
                  </Button>
                </div>
                <OltpTenantPanel orgId={selectedRow.org_id} />
              </div>
            )}
          </div>
        )}
      </AdminAsync>
    </AdminPage>
  );
};

export default AdminOltp;
