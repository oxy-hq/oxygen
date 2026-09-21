import { Link } from "react-router-dom";
import { useFleetStorage } from "@/hooks/api/customApps/useAppStorage";
import { cn } from "@/libs/shadcn/utils";
import { timeAgo } from "@/libs/utils/date";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { CustomApp } from "@/types/apps";
import { ConsolePanel, PanelRow } from "../ConsolePanel";
import { formatBytes, formatDelta } from "../StorageTab/utils";

/**
 * What this one app is costing in storage — Q5, answered without leaving the console.
 *
 * Storage used to be a top-level tab: a sibling of "the apps", as if an app's disk usage
 * were a different kind of thing from the app. It is a fact *about* an app, so it is a
 * panel on the app, and the cross-fleet totals that genuinely are deployment-level live
 * behind the audit link at the foot of the console.
 *
 * Reads the fleet endpoint and picks this app's row rather than asking for one app: the
 * same query backs the audit table, so switching between apps is served from cache
 * instead of a request per app.
 */
export const AppStoragePanel = ({ app }: { app: CustomApp }) => {
  const fleet = useFleetStorage();

  return (
    <ConsolePanel id='storage' title='Storage' question='Q5 — what it is costing'>
      <AdminAsync query={fleet} noun="this app's storage" rows={3}>
        {(data) => {
          const row = data.rows.find((r) => r.appId === app.id);
          if (!row) {
            // No usage row is "never measured", not "zero bytes" — the distinction the
            // fleet response draws with `unmeasuredApps`, and worth keeping: reporting
            // 0 GiB for an app the sweeper has not reached is a number someone would act on.
            return (
              <p
                className='text-muted-foreground text-xs'
                data-testid='apps-console-storage-unmeasured'
              >
                Never measured. The storage sweeper has not walked this app yet — that is not the
                same as it holding nothing.
              </p>
            );
          }
          const partial = row.measureStatus !== "ok";
          return (
            <div className='flex flex-col gap-0.5'>
              <PanelRow label='Size' mono data-testid='apps-console-storage-size'>
                {formatBytes(row.bytes)}
              </PanelRow>
              <PanelRow label='Objects' mono>
                {row.objectCount.toLocaleString()}
              </PanelRow>
              <PanelRow label='7-day change' mono>
                {row.growthBytes7d === null ? (
                  // Null means no sample old enough to difference against — say so
                  // rather than printing a 0 that reads as "flat".
                  <span className='text-muted-foreground'>no baseline yet</span>
                ) : (
                  formatDelta(row.growthBytes7d)
                )}
              </PanelRow>
              <PanelRow label='No retention rule' mono>
                {row.untaggedBytes > 0 ? (
                  <span className={ADMIN_TONE.warn.text}>{formatBytes(row.untaggedBytes)}</span>
                ) : (
                  "—"
                )}
              </PanelRow>
              <PanelRow label='Last measured' mono>
                <span title={new Date(row.measuredAt).toLocaleString()}>
                  {timeAgo(row.measuredAt)}
                </span>
              </PanelRow>

              {partial ? (
                <p
                  data-testid='apps-console-storage-partial'
                  className={cn(
                    "mt-2 rounded-md px-2 py-1.5 text-xs",
                    ADMIN_TONE.warn.bg,
                    ADMIN_TONE.warn.text
                  )}
                >
                  The last walk was <span className='font-medium'>{row.measureStatus}</span> — these
                  numbers are a floor, not a total.
                  {row.measureDetail ? ` ${row.measureDetail}` : ""}
                </p>
              ) : null}

              <p className='mt-2 text-muted-foreground text-xs'>
                <Link
                  to='/admin/apps/storage'
                  className='underline underline-offset-2 hover:text-foreground'
                  data-testid='apps-console-storage-fleet-link'
                >
                  Compare across the fleet
                </Link>
              </p>
            </div>
          );
        }}
      </AdminAsync>
    </ConsolePanel>
  );
};
