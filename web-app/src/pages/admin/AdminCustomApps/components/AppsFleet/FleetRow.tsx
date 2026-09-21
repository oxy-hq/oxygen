import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AppHealthRow, AppStorageUsageRow, CustomApp } from "@/types/apps";
import { type AppStatus, STATUS_LABEL } from "../../appStatus";
import type { FleetColumn } from "../../fleetColumns";
import { relativeTime } from "../AppDetail/components/Activity/relativeTime";
import { AppIdentity } from "../AppIdentity";
import { statusTone } from "../AppSwitcher";
import { formatBytes } from "../StorageTab/utils";

/**
 * One app in the fleet.
 *
 * The whole row is a link to that app's console, because the fleet's job is to hand an
 * operator to the app that needs them — not to be a place they work. The old table put
 * a kebab menu of eight actions on every row, which is how a list becomes a surface you
 * live in rather than pass through.
 *
 * Cells are rendered from the derived column set, so a column this deployment cannot
 * answer never reaches here. That is why there is no `—` fallback in most cells: an
 * absent value is a missing *column*, handled once, rather than a dash repeated per row.
 * The exception is a value that is genuinely per-app optional (one app has
 * `last_active_at`, another does not) — that dash is information.
 */
export const FleetRow = ({
  app,
  status,
  columns,
  health,
  storage
}: {
  app: CustomApp;
  status: AppStatus | null;
  columns: readonly FleetColumn[];
  health: AppHealthRow | undefined;
  storage: AppStorageUsageRow | undefined;
}) => {
  const tone = status === null ? "muted" : statusTone(status);

  const cell = (column: FleetColumn) => {
    switch (column.id) {
      case "status":
        return (
          <span className='flex items-center gap-1.5'>
            <span className={cn("size-1.5 shrink-0 rounded-full", ADMIN_TONE[tone].dot)} />
            <span className={cn("truncate", ADMIN_TONE[tone].text)}>
              {/* `null` is "not known yet" and must not borrow a verdict. */}
              {status === null ? "Unknown" : STATUS_LABEL[status]}
            </span>
          </span>
        );
      case "app":
        return <AppIdentity app={app} />;
      case "published":
        return app.published_at ? (
          <span className='text-muted-foreground'>{relativeTime(app.published_at)}</span>
        ) : (
          <span className='text-muted-foreground/60'>Draft</span>
        );
      case "lastActive":
        return app.last_active_at ? (
          <span className='text-muted-foreground'>{relativeTime(app.last_active_at)}</span>
        ) : (
          <span className='text-muted-foreground/50'>—</span>
        );
      case "requests":
        // Only rendered when capture is on, so these numbers mean something.
        // No health row means this app fell past the fleet endpoint's page cap, not
        // that it served nothing. `0` there would contradict the Status cell on the
        // same row, which correctly reads "Unknown".
        return health ? (
          <span className='tabular-nums'>
            {health.requests}
            {health.failed > 0 ? (
              <span className={cn("ml-1", ADMIN_TONE.danger.text)}>{health.failed} failed</span>
            ) : null}
          </span>
        ) : (
          <span className='text-muted-foreground/50'>—</span>
        );
      case "storage":
        return storage ? (
          <span className='tabular-nums'>
            {formatBytes(storage.bytes)}
            {storage.measureStatus !== "ok" ? (
              <span
                className={cn("ml-1", ADMIN_TONE.warn.text)}
                title={storage.measureDetail ?? ""}
              >
                floor
              </span>
            ) : null}
          </span>
        ) : (
          <span className='text-muted-foreground/50'>—</span>
        );
    }
  };

  return (
    <Link
      to={`/admin/apps/${app.org_slug}/${app.slug}`}
      data-testid={`admin-apps-fleet-row-${app.org_slug}-${app.slug}`}
      className='col-span-full grid grid-cols-subgrid items-center border-b px-4 py-2 text-xs transition-colors last:border-b-0 hover:bg-muted/40'
    >
      {columns.map((c) => (
        <span key={c.id} className={cn("min-w-0", c.numeric && "text-right")}>
          {cell(c)}
        </span>
      ))}
    </Link>
  );
};

/**
 * The tracks, declared **once on the scroller** — the header and every row are
 * `grid-cols-subgrid` children of that one container, so they resolve the *same*
 * tracks rather than each computing its own.
 *
 * This used to be applied to each row's own `display:grid`, with a comment claiming the
 * header and rows "cannot drift apart". They could, and did: `max-content` is sized from
 * the items in *its own* container, so four grid containers produced four layouts. Header
 * `PUBLISHED` began at x=1350 while the three rows began at 1414, 1434 and 1469 — a
 * 119px spread. It read as fine on the seeded fleet only because three similar-length
 * values happen to resolve to similar widths, which is exactly why looking at it did not
 * catch it; measuring the resolved `gridTemplateColumns` per container did.
 *
 * It also silently voided the point of `numeric`: right-aligning to a track whose right
 * edge moves per row is not a column anyone can read down, which is the entire reason
 * Requests and Storage are right-aligned and tabular.
 *
 * `App` takes the slack because it is the only cell whose content has no bound.
 */
export const gridTemplate = (columns: readonly FleetColumn[]): string =>
  columns.map((c) => (c.id === "app" ? "minmax(0,1fr)" : "max-content")).join(" ");
