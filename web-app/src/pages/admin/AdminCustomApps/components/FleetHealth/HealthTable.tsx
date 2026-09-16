import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import type { AppHealthRow } from "@/types/apps";
import { HealthBadge } from "./HealthDot";

/** `360` → `6h`, for a column header that has to stay narrow. */
const hours = (minutes: number) => `${Math.round(minutes / 60)}h`;

export const HealthTable = ({ apps }: { apps: AppHealthRow[] }) => {
  if (apps.length === 0) {
    return (
      <div
        className='rounded-md border border-dashed p-8 text-center'
        data-testid='admin-fleet-health-empty'
      >
        <p className='text-muted-foreground text-sm'>No apps match this filter.</p>
      </div>
    );
  }

  const window = apps[0].window_minutes;
  return (
    <div className='overflow-x-auto rounded-md border' data-testid='admin-fleet-health-table'>
      <table className='w-full text-xs'>
        <thead className='border-b bg-muted/40 text-muted-foreground'>
          <tr>
            <th className='px-3 py-2 text-left font-medium'>App</th>
            <th className='px-3 py-2 text-left font-medium'>Org</th>
            <th className='px-3 py-2 text-left font-medium'>Health</th>
            <th className='px-3 py-2 text-right font-medium'>Requests ({hours(window)})</th>
            <th className='px-3 py-2 text-right font-medium'>Failed</th>
            <th className='px-3 py-2 text-left font-medium'>What we know</th>
          </tr>
        </thead>
        <tbody>
          {apps.map((app) => (
            <tr
              key={app.app_id}
              className='border-b last:border-0 hover:bg-muted/30'
              data-testid={`admin-fleet-health-row-${app.app_slug}`}
            >
              {/* The org slug comes from a lookup that is allowed to fail — a
                  missing slug costs a display name, not a verdict. Without it
                  the detail route resolves to `/admin/apps//slug`, which lands
                  nowhere, so show plain text rather than a link that lies. */}
              <td className='px-3 py-2'>
                {app.org_slug ? (
                  <Link
                    to={`/admin/apps/${app.org_slug}/${app.app_slug}`}
                    className='font-medium hover:underline'
                  >
                    {app.app_name || app.app_slug}
                  </Link>
                ) : (
                  <span className='font-medium'>{app.app_name || app.app_slug}</span>
                )}
              </td>
              <td className='px-3 py-2 text-muted-foreground'>{app.org_slug}</td>
              <td className='px-3 py-2'>
                <HealthBadge health={app.health} />
              </td>
              {/* Counts are meaningless on an unmeasured app — an em dash says
                  "not known", where a 0 would claim the app served nothing. */}
              <td className='px-3 py-2 text-right tabular-nums'>
                {app.health === "not_measured" ? (
                  <span className='text-muted-foreground'>—</span>
                ) : (
                  app.requests.toLocaleString()
                )}
              </td>
              <td
                className={cn(
                  "px-3 py-2 text-right tabular-nums",
                  app.failed > 0 && app.health !== "not_measured" && "text-destructive"
                )}
              >
                {app.health === "not_measured" ? (
                  <span className='text-muted-foreground'>—</span>
                ) : (
                  app.failed.toLocaleString()
                )}
              </td>
              <td className='max-w-md px-3 py-2 text-muted-foreground'>{app.reason ?? ""}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
};
