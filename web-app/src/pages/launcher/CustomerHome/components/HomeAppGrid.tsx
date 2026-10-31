import { useAppUnreadCounts, useLatestUnreadByApp } from "@/hooks/api/notifications";
import { cn } from "@/libs/shadcn/utils";
import type { CustomAppSummary } from "@/types/apps";
import { HomeAppTile } from "./HomeAppTile";

/**
 * The home's apps: a fixed place that widgets never push down. Four icons to a
 * phone's row; from `sm` up, row tiles, two across beside the Today rail, up to
 * four without it. The apps' column scrolls on its own, so the rail stays put.
 */
export function HomeAppGrid({
  apps,
  besideRail,
  onManageAccess
}: {
  apps: CustomAppSummary[];
  /** The Today rail takes the right third: two columns of tiles, not three or four. */
  besideRail: boolean;
  onManageAccess?: (app: CustomAppSummary) => void;
}) {
  const unread = useAppUnreadCounts();
  const latest = useLatestUnreadByApp();
  return (
    <div
      className={cn(
        "grid grid-cols-4 gap-x-2 gap-y-4 sm:grid-cols-2 sm:gap-3 lg:grid-cols-3",
        // Beside the rail a third column would cut every name and freshness line short.
        besideRail ? "lg:grid-cols-2" : "xl:grid-cols-4"
      )}
      data-testid='launcher-app-grid'
      data-beside-rail={besideRail}
    >
      {apps.map((app) => (
        <HomeAppTile
          key={app.id}
          app={app}
          unread={unread[app.id] ?? 0}
          latest={latest[app.id]}
          onManageAccess={onManageAccess}
        />
      ))}
    </div>
  );
}
