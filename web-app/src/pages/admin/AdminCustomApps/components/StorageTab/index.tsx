import { Loader2, RefreshCw } from "lucide-react";
import { useMemo, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { useFleetStorage, useSweepStorage } from "@/hooks/api/customApps/useAppStorage";
import { AdminDetailStats } from "../../../components/AdminDetailStats";
import { AppBrowser } from "./components/AppBrowser";
import { FleetTable } from "./components/FleetTable";
import { type Range, UsageChart } from "./components/UsageChart";
import { formatBytes, formatDelta } from "./utils";

type SortKey = "bytes" | "growth" | "untagged";

/**
 * Admin → Apps → Storage.
 *
 * Reads top-down the way the question is actually asked: **is the fleet
 * growing** (stat tiles + trend), **who is responsible** (ranked table), **what
 * is in there and can I delete it** (per-app trend + browser).
 *
 * Three sources, deliberately. The tiles and table read the sweeper's rollup —
 * ranking every app cannot mean walking every S3 prefix per request. The charts
 * read the sample series, because a level going up and to the right is exactly
 * what a table of current sizes cannot show. The browser reads S3 live, because
 * the rollup holds no per-object rows and an operator investigating *now* needs
 * current truth rather than a listing up to a sweep old.
 *
 * See `internal-docs/customer-apps-functions.md` ("Quotas, usage & the admin Storage tab").
 */
export default function StorageTab() {
  const [sort, setSort] = useState<SortKey>("bytes");
  const [days, setDays] = useState<Range>(30);
  const [selectedAppId, setSelectedAppId] = useState<string | null>(null);
  const { data, isLoading } = useFleetStorage(sort);
  const sweep = useSweepStorage();

  const selected = useMemo(
    () => data?.rows.find((r) => r.appId === selectedAppId) ?? null,
    [data, selectedAppId]
  );

  // Fleet-wide 7-day change, summed from the rows that HAVE a baseline. Apps too
  // new to difference report null; counting those as zero would understate the
  // fleet's movement rather than admit the figure is partial.
  const growth7d = useMemo(() => {
    const known = data?.rows.filter((r) => r.growthBytes7d !== null) ?? [];
    if (known.length === 0) return null;
    return known.reduce((sum, r) => sum + (r.growthBytes7d ?? 0), 0);
  }, [data]);

  const total = data?.totalBytes ?? 0;
  const untagged = data?.totalUntaggedBytes ?? 0;
  const stats = useMemo(
    () => [
      { label: "Total stored", value: formatBytes(total) },
      {
        label: "7-day change",
        // Growth is the number that predicts the invoice, so it is the one that
        // gets a tone — but only when it is actually growing.
        value: (
          <span className={growth7d !== null && growth7d > 0 ? "text-warning" : undefined}>
            {formatDelta(growth7d)}
          </span>
        )
      },
      { label: "Objects", value: (data?.totalObjects ?? 0).toLocaleString() },
      {
        label: "No retention rule",
        // Zero is the good state and must read as unremarkable, not as an alert.
        value: (
          <span className={untagged > 0 ? "text-warning" : undefined}>{formatBytes(untagged)}</span>
        ),
        sub: untagged > 0 && total > 0 ? `${Math.round((untagged / total) * 100)}% of stored` : null
      }
    ],
    [data, growth7d, total, untagged]
  );

  return (
    <div className='flex min-h-0 flex-1 flex-col overflow-auto' data-testid='admin-storage-tab'>
      <header className='flex items-center justify-between border-b p-2'>
        <div>
          <h2 className='font-semibold text-sm'>Custom-app storage</h2>
          <p className='text-muted-foreground text-xs'>
            {data?.rows.length ?? 0} measured app{data?.rows.length === 1 ? "" : "s"}
            {data?.softLimitBytes
              ? ` · soft limit ${formatBytes(data.softLimitBytes)} per org`
              : null}
          </p>
        </div>
        <Button
          variant='outline'
          size='sm'
          onClick={() => sweep.mutate()}
          disabled={sweep.isPending}
          data-testid='admin-storage-measure-now'
        >
          <RefreshCw className={sweep.isPending ? "mr-1 size-3 animate-spin" : "mr-1 size-3"} />
          Measure now
        </Button>
      </header>

      {/* Two caveats that would otherwise silently misrepresent the totals. */}
      {(data?.unmeasuredApps ?? 0) > 0 && (
        <p className='border-b bg-muted/40 px-3 py-2 text-muted-foreground text-xs'>
          {data?.unmeasuredApps} app(s) have never been measured — they are missing from these
          totals, not empty.
        </p>
      )}
      {data?.totalsAreFloor && (
        <p className='border-b bg-muted/40 px-3 py-2 text-warning text-xs'>
          At least one app’s last walk was incomplete, so these totals are a floor.
        </p>
      )}

      <div className='space-y-2 p-2' data-testid='admin-storage-stats'>
        <AdminDetailStats items={stats} />
        <UsageChart days={days} onDaysChange={setDays} title='Stored over time' id='fleet' />
      </div>

      <div className='flex min-h-0 flex-1 border-t'>
        <div className='min-w-0 flex-1 overflow-auto border-r'>
          {isLoading ? (
            <div className='flex items-center gap-2 p-2 text-muted-foreground text-xs'>
              <Loader2 className='size-3 animate-spin' /> Loading usage…
            </div>
          ) : (
            <FleetTable
              rows={data?.rows ?? []}
              sort={sort}
              onSortChange={setSort}
              selectedAppId={selectedAppId}
              onSelect={setSelectedAppId}
            />
          )}
        </div>
        <div className='flex min-w-0 flex-1 flex-col overflow-auto'>
          {selected ? (
            <>
              <div className='border-b p-2'>
                {/* Same chart, scoped to one app — so "the fleet is growing" and
                    "this app is why" are read exactly the same way. The range
                    is shared, so switching apps keeps the window. */}
                <UsageChart
                  days={days}
                  onDaysChange={setDays}
                  appId={selected.appId}
                  title={selected.appName || selected.appSlug}
                  height={120}
                  id='app'
                />
              </div>
              <AppBrowser app={selected} />
            </>
          ) : (
            <p className='p-2 text-muted-foreground text-xs'>
              Select an app to see its trend and clean up its assets.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}
