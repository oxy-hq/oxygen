import { useAppUnreadCounts, useLatestUnreadByApp } from "@/hooks/api/notifications";
import type { CustomAppSummary } from "@/types/apps";
import { HomeAppTile } from "./HomeAppTile";

/**
 * The home's apps: a fixed place that widgets never push down. The columns follow
 * the room the grid has, not the screen (container queries: the section around it
 * is the container), so it fills its column beside the side column or the whole
 * width without one, with no flag passed down. Four icons to a row on a phone;
 * from 24rem, row tiles: two, three from 42rem, four from 64rem.
 */
export function HomeAppGrid({
  apps,
  onManageAccess,
  large
}: {
  apps: CustomAppSummary[];
  onManageAccess?: (app: CustomAppSummary) => void;
  /** Home v3's bigger tiles. */
  large?: boolean;
}) {
  const unread = useAppUnreadCounts();
  const latest = useLatestUnreadByApp();
  return (
    <div
      className='grid @2xl:grid-cols-3 @5xl:grid-cols-4 @sm:grid-cols-2 grid-cols-4 @sm:gap-3 gap-x-2 gap-y-4'
      data-testid='launcher-app-grid'
    >
      {apps.map((app) => (
        <HomeAppTile
          key={app.id}
          app={app}
          unread={unread[app.id] ?? 0}
          latest={latest[app.id]}
          onManageAccess={onManageAccess}
          large={large}
        />
      ))}
    </div>
  );
}
