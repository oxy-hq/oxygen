import { Activity as ActivityIcon } from "lucide-react";
import { useState } from "react";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import {
  useAppActivityEventGroups,
  useAppActivityEventOccurrences
} from "@/hooks/api/customApps/useCustomApps";
import { AdminInfo } from "@/pages/admin/components/AdminInfo";
import { relativeTime } from "../relativeTime";

/**
 * Engineer-tagged custom events grouped by name, with click-to-drill-down into
 * recent occurrences.
 */
export const ActivityEvents = ({ appId }: { appId: string }) => {
  const [drillName, setDrillName] = useState<string | null>(null);
  const groups = useAppActivityEventGroups(appId, 7);
  const occurrences = useAppActivityEventOccurrences(appId, drillName, 7);

  return (
    <section data-testid='admin-app-activity-events'>
      <h3 className='mb-2 flex items-center gap-1.5 font-medium text-muted-foreground text-xs uppercase tracking-wider'>
        <ActivityIcon className='size-3.5' />
        Events (last 7 days)
      </h3>
      {groups.isLoading ? (
        <Skeleton className='h-24 w-full' />
      ) : (groups.data?.length ?? 0) === 0 ? (
        <p
          className='flex items-center gap-1.5 text-muted-foreground text-xs'
          data-testid='admin-app-activity-events-empty'
        >
          No custom events yet.
          <AdminInfo>
            The bundle logs interactions with <code>useTrackEvent("name", payload)</code>; they
            appear here grouped by name.
          </AdminInfo>
        </p>
      ) : (
        <div className='overflow-hidden rounded-md border'>
          <table className='w-full text-xs'>
            <thead className='bg-muted/40 text-muted-foreground text-xs uppercase tracking-wider'>
              <tr>
                <th className='px-3 py-2 text-left font-medium'>Event</th>
                <th className='px-3 py-2 text-right font-medium'>Count</th>
                <th className='px-3 py-2 text-right font-medium'>Last fired</th>
              </tr>
            </thead>
            <tbody>
              {groups.data?.map((g) => (
                <tr
                  key={g.event_name}
                  className='cursor-pointer border-t hover:bg-accent/40'
                  data-testid={`admin-app-activity-event-${g.event_name}`}
                  onClick={() => setDrillName(drillName === g.event_name ? null : g.event_name)}
                >
                  <td className='px-3 py-2 font-mono text-xs'>{g.event_name}</td>
                  <td className='px-3 py-2 text-right'>{g.count}</td>
                  <td className='px-3 py-2 text-right text-muted-foreground text-xs'>
                    {relativeTime(g.last_fired_at)}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {drillName && (
        <div
          className='mt-3 rounded-md border bg-muted/20 p-3'
          data-testid='admin-app-activity-event-drilldown'
        >
          <div className='mb-2 flex items-center justify-between'>
            <span className='font-mono text-xs'>{drillName} — recent occurrences</span>
            <button
              type='button'
              className='text-muted-foreground text-xs hover:text-foreground'
              onClick={() => setDrillName(null)}
            >
              Close
            </button>
          </div>
          {occurrences.isLoading ? (
            <Skeleton className='h-32 w-full' />
          ) : (occurrences.data?.length ?? 0) === 0 ? (
            <p className='text-muted-foreground text-xs'>No occurrences in the window.</p>
          ) : (
            <div className='space-y-2'>
              {occurrences.data?.map((o) => (
                <div key={o.id} className='rounded border bg-background p-2'>
                  <div className='flex items-center justify-between font-mono text-muted-foreground text-xs'>
                    <span>{o.user_email}</span>
                    <span>{relativeTime(o.occurred_at)}</span>
                  </div>
                  <pre className='mt-1 overflow-auto whitespace-pre-wrap font-mono text-xs'>
                    {JSON.stringify(o.payload, null, 2)}
                  </pre>
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </section>
  );
};
