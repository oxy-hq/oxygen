import { useState } from "react";
import { eyebrow } from "@/components/ui/reference/tokens";
import { cn } from "@/libs/shadcn/utils";
import type { CustomAppSummary } from "@/types/apps";
import { FirstAppsOnTheirWay } from "../FirstAppsOnTheirWay";
import { AppPreview } from "./AppPreview";
import { type AppSort, AppsSort, sortApps } from "./AppsSort";

/**
 * The home's apps: the eyebrow with the sort at its right end, then every app the
 * person can open across the org as a preview card. On a desk the grid's columns
 * follow the section's own width (a container query), so opening the rail or the
 * notifications panel reflows it; on a phone it is one card to a row, so each
 * name has the width.
 *
 * Transcribed from oxy-hq/reference-app `src/views/parts/HomeApps.tsx`
 * (`HomeAppsSection`, branch design/home-match-oxygen).
 */
export function HomeAppsSection({
  apps,
  recent,
  phone,
  orgName,
  onManage
}: {
  apps: readonly CustomAppSummary[];
  /** The ones opened last from here, newest first ("Recently used"). */
  recent: readonly CustomAppSummary[];
  phone: boolean;
  orgName: string | undefined;
  onManage?: (app: CustomAppSummary) => void;
}) {
  const [sort, setSort] = useState<AppSort>("recent");
  const sorted = sortApps(apps, recent, sort);
  return (
    <section className='@container flex flex-col gap-3' aria-labelledby='home-apps-heading'>
      <div className='flex items-center justify-between gap-3 py-1'>
        <h2 id='home-apps-heading' className={cn(eyebrow, "m-0")}>
          Your apps
        </h2>
        {apps.length > 1 && <AppsSort sort={sort} onSort={setSort} />}
      </div>
      {apps.length === 0 ? (
        <FirstAppsOnTheirWay orgName={orgName} />
      ) : (
        <div
          className={cn(
            "grid",
            phone
              ? "grid-cols-1 gap-3"
              : // Three columns from a 48rem container, not 42rem: at 42rem a card's name has ~137px.
                "grid-cols-1 gap-4 @3xl:grid-cols-3 @5xl:grid-cols-4 @sm:grid-cols-2"
          )}
          data-testid='launcher-app-grid'
        >
          {sorted.map((app) => (
            <AppPreview key={app.id} app={app} onManage={onManage} />
          ))}
        </div>
      )}
    </section>
  );
}
