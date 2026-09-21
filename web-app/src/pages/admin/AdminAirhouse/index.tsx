import { Search } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import { Input } from "@/components/ui/shadcn/input";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { Table, TableBody, TableHeader, TableRow } from "@/components/ui/shadcn/table";
import { useAirhouseFleet } from "@/hooks/api/airhouse/useAdminAirhouse";
import { AdminAsync } from "../components/AdminAsync";
import { AdminPage } from "../components/AdminPage";
import { AdminSectionLabel } from "../components/AdminSectionLabel";
import { ADMIN_HEADER_ROW_CLASS, AdminTh } from "../components/AdminTable";
import { AirhouseFleetRow } from "./components/AirhouseFleetRow";
import { FleetFilterChips } from "./components/FleetFilterChips";
import { UnprovisionedSection } from "./components/UnprovisionedSection";
import { bySeverityThenName, countBySeverity, type Severity, severityOf } from "./severity";

/**
 * Below this many, the "No warehouse" list opens by default: a collapse that
 * hides four rows costs a click and saves nothing.
 */
const UNPROVISIONED_INLINE_MAX = 8;

/**
 * The Airhouse fleet: which workspaces have a warehouse, and is anything wrong.
 *
 * **A triage queue, not a list.** The page is read when something is broken, so
 * it is ordered by severity rather than by name: the tenants that cannot serve a
 * query are on screen before the operator does anything. Filtering is the same
 * decision — the summary counts *are* the filter, so "2 without a service
 * account" is one click from those two rows instead of a number to go hunting
 * with.
 *
 * **Workspace-keyed.** An Airhouse tenant is one per workspace rather than one
 * per org, so the org is a column here rather than the key.
 *
 * **Rows open in place rather than into a side panel.** A panel costs
 * horizontal room for as long as it is open, and this page's job is to show as
 * much of the fleet at once as it can; opening downward spends space only on
 * the row being investigated and keeps its neighbours on screen for comparison.
 * What the strip holds is the psql session an operator would otherwise open —
 * the service account id, the service account's role and lifetime *ceilings*,
 * and the three dates. All of it was already on the row this page loads and was
 * being discarded by the API.
 */
const AdminAirhouse = () => {
  // The whole query: `AdminAsync` needs it to tell a fleet that is still
  // loading from one that failed to load from one that is genuinely empty.
  const fleet = useAirhouseFleet();
  const { data } = fleet;
  const [query, setQuery] = useState("");
  const [severity, setSeverity] = useState<Severity | null>(null);
  const [openRow, setOpenRow] = useState<string | null>(null);
  // `null` until the operator decides, so the default can depend on how big
  // the list turns out to be.
  const [unprovisionedOpen, setUnprovisionedOpen] = useState<boolean | null>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  // `/` focuses the filter, the way every tool an operator already has open
  // does. Ignored while typing, so it still reaches a text field.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "/" || e.metaKey || e.ctrlKey || e.altKey) return;
      const el = e.target as HTMLElement | null;
      if (el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable))
        return;
      e.preventDefault();
      searchRef.current?.focus();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const { live, empty, counts, provisionedTotal, total, unprovisionedTotal } = useMemo(() => {
    const all = data?.rows ?? [];
    const q = query.trim().toLowerCase();
    const match = (r: (typeof all)[number]) =>
      !q ||
      r.workspace_name.toLowerCase().includes(q) ||
      r.org_name.toLowerCase().includes(q) ||
      r.tenant_id.toLowerCase().includes(q) ||
      r.bucket.toLowerCase().includes(q);

    const provisioned = all.filter((r) => r.status !== "none");
    return {
      // Counts come from the whole provisioned fleet, not the filtered view —
      // a chip that renumbered itself as you filtered would make the fleet look
      // like it changed when only the question did.
      counts: countBySeverity(provisioned),
      provisionedTotal: provisioned.length,
      total: all.length,
      live: provisioned
        .filter(match)
        .filter((r) => severity === null || severityOf(r) === severity)
        .sort(bySeverityThenName),
      empty: all.filter((r) => r.status === "none" && match(r)),
      // The unfiltered size — see `showUnprovisioned`.
      unprovisionedTotal: all.length - provisioned.length
    };
  }, [data, query, severity]);

  // Starting a search forgets an earlier explicit collapse. Without this, an
  // operator who opened the section to browse and then closed it pinned it shut
  // for the rest of the session — and every later search left the rows it
  // matched behind a disclosure, which is the "the search found nothing"
  // problem the rule below exists to prevent, back permanently and silently.
  const searching = query.trim() !== "";
  useEffect(() => {
    if (searching) setUnprovisionedOpen(null);
    // `[searching]` is a boolean, so this fires on the TRANSITION into a search
    // and not on every keystroke — which is what makes a collapse chosen
    // *during* one hold. (A ref tracking the previous value was doing the same
    // job twice; the dependency already is the edge.)
  }, [searching]);

  // Open when collapsing earns nothing: a short list, or one the operator has
  // already narrowed themselves — they asked for exactly those rows, so leaving
  // them behind a disclosure makes the search look like it found nothing. An
  // explicit click still wins over both.
  //
  // The threshold reads the UNFILTERED count on purpose. With the query clause
  // beside it the two happen to coincide — when there is no query, `empty` is
  // unfiltered — so swapping in `empty.length` would pass every test here. It
  // would also re-introduce, the moment that clause is touched, the behaviour
  // this pair replaced: a collapsed 200-row section springing open as a query
  // passed eight and re-collapsing when it cleared, which is the fleet
  // appearing to change when only the question did.
  const showUnprovisioned =
    unprovisionedOpen ?? (searching || unprovisionedTotal <= UNPROVISIONED_INLINE_MAX);

  // `FleetTruncation.any()` is a Rust method and does not cross the wire.
  const anyTruncated = Boolean(data?.truncated?.unprovisioned || data?.truncated?.provisioned);

  return (
    <AdminPage
      width='wide'
      // The count goes in `actions`, not `description`: it is live data, and in
      // the `description` slot it would have to render *something* before the
      // fleet arrives — "0 of 0 workspaces provisioned" is a claim, not a
      // placeholder. Here it is simply absent until there is a fleet to count.
      // It also keeps the filter box in the top-right corner it already had.
      actions={
        <>
          {data ? (
            <span
              className='text-muted-foreground text-xs tabular-nums'
              data-testid='admin-airhouse-count'
            >
              {provisionedTotal} of {anyTruncated ? `the first ${total}` : total} workspaces
              provisioned
            </span>
          ) : null}
          <div className='relative w-64'>
            <Search className='absolute top-1/2 left-2 size-3 -translate-y-1/2 text-muted-foreground' />
            <Input
              className='h-7 pl-7 text-xs'
              placeholder='Workspace, org, tenant or bucket…'
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              data-testid='admin-airhouse-filter'
            />
            {!query && (
              <kbd className='pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 rounded border border-border/60 px-1 font-mono text-[10px] text-muted-foreground'>
                /
              </kbd>
            )}
          </div>
        </>
      }
      data-testid='admin-airhouse'
    >
      <AdminAsync
        query={fleet}
        noun='the Airhouse fleet'
        // One block, not bars: this page is a triage queue whose first paint is
        // a summary strip plus a table, and row bars would mis-promise it.
        skeleton={<Skeleton className='h-64 w-full' />}
      >
        {(loaded) => (
          <div className='flex flex-col gap-3'>
            {/* Report and act in one control — see FleetFilterChips. */}
            <FleetFilterChips
              counts={counts}
              total={provisionedTotal}
              active={severity}
              onChange={setSeverity}
            />

            {/* Which half was cut decides the words. The cap normally falls on
                workspaces without a warehouse, so every provisioned tenant is on
                the page — but when the provisioned half hits its own cap, a
                warehouse may be missing, and the first sentence would assert the
                opposite of what happened.

                The filter is client-side: it searches the rows already returned,
                so telling an operator to "narrow it" would point them at the one
                action that cannot reach a truncated row, and would look like it
                worked. */}
            {(loaded.truncated?.provisioned || loaded.truncated?.unprovisioned) && (
              <p className='text-warning text-xs' data-testid='admin-airhouse-truncated'>
                {loaded.truncated.provisioned
                  ? "More rows exist than are shown, including workspaces that have a warehouse."
                  : `Every provisioned workspace is shown; the ones without a warehouse are the first ${total - provisionedTotal} by name.`}
              </p>
            )}

            <div className='flex flex-col gap-2' data-testid='admin-airhouse-provisioned'>
              <AdminSectionLabel trailing={String(live.length)}>Provisioned</AdminSectionLabel>
              {live.length === 0 ? (
                <p
                  className='px-1 py-2 text-muted-foreground text-xs'
                  data-testid='admin-airhouse-provisioned-empty'
                >
                  {severity || query
                    ? "No warehouse matches that filter."
                    : "No workspace has a warehouse yet."}
                </p>
              ) : (
                <Table>
                  <TableHeader>
                    <TableRow className={ADMIN_HEADER_ROW_CLASS}>
                      <AdminTh>Workspace</AdminTh>
                      <AdminTh>Org</AdminTh>
                      <AdminTh>Tenant</AdminTh>
                      <AdminTh>Storage</AdminTh>
                      <AdminTh align='right'>SA rotated</AdminTh>
                      <AdminTh align='right'>Status</AdminTh>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {live.map((r) => (
                      <AirhouseFleetRow
                        key={r.workspace_id}
                        row={r}
                        expanded={openRow === r.workspace_id}
                        onToggle={() =>
                          setOpenRow((cur) => (cur === r.workspace_id ? null : r.workspace_id))
                        }
                      />
                    ))}
                  </TableBody>
                </Table>
              )}
            </div>

            {/* Hidden entirely while a severity filter is on: severity is a
                property of a provisioned tenant, so a list of workspaces that
                have none is not an answer to "show me the broken ones" — it is
                the rest of the page refusing to narrow. */}
            {severity === null && empty.length > 0 && (
              <UnprovisionedSection
                rows={empty}
                open={showUnprovisioned}
                onToggle={() => setUnprovisionedOpen(!showUnprovisioned)}
              />
            )}
          </div>
        )}
      </AdminAsync>
    </AdminPage>
  );
};

export default AdminAirhouse;
