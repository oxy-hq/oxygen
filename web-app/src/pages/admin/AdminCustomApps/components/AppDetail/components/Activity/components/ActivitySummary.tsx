import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useAppActivitySummary } from "@/hooks/api/customApps/useCustomApps";
import { AdminKpiStrip } from "@/pages/admin/components/AdminKpiStrip";
import { relativeTime } from "../relativeTime";

/**
 * The "is anyone using this?" answer at a glance: one strip of four numbers, zeros
 * faded, rather than four bordered boxes that all read 0 on a quiet app.
 *
 * It renders in the console's Activity page and in the dossier docked beside a preview,
 * which can be 300px wide on a 1440px screen. `AdminKpiStrip` lays its columns out by
 * its own width for that reason: two by two there, four across in the console.
 */
export const ActivitySummary = ({ appId }: { appId: string }) => {
  const { data, isLoading } = useAppActivitySummary(appId);

  if (isLoading) {
    return <Skeleton className='h-[4.5rem] w-full' />;
  }

  const users = data?.unique_users_7d ?? 0;
  const views = data?.total_views_7d ?? 0;
  const events = data?.total_events_7d ?? 0;
  return (
    <div data-testid='admin-app-activity-summary'>
      <AdminKpiStrip
        data-testid='admin-app-activity-stat'
        items={[
          {
            id: "last-viewed",
            label: "Last viewed",
            value: data?.last_viewed_at ? relativeTime(data.last_viewed_at) : "never",
            quiet: !data?.last_viewed_at
          },
          {
            id: "unique-users",
            label: "Users",
            value: users,
            caption: "last 7 days",
            quiet: users === 0
          },
          {
            id: "total-views",
            label: "Views",
            value: views,
            caption: "last 7 days",
            quiet: views === 0
          },
          {
            id: "total-events",
            label: "Events",
            value: events,
            caption: "last 7 days",
            quiet: events === 0
          }
        ]}
      />
    </div>
  );
};
