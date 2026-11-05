import type { ReactNode } from "react";
import type { CustomAppSummary } from "@/types/apps";
import type { HomeView } from "../useHomeView";
import { FirstAppsOnTheirWay } from "./FirstAppsOnTheirWay";
import { HomeAppGrid } from "./HomeAppGrid";
import { HomeAppIcons } from "./HomeAppIcons";

/** Fewer apps than this fit on one screen anyway: a recent row would only repeat them. */
const RECENT_FROM = 9;

const HEADING = "font-medium text-muted-foreground text-sm";

/**
 * The home's apps, the page's main thing. Simple: big icons with their names, the
 * ones opened last in a row above the rest once there are enough apps to look for
 * one. Full: tiles that also say how fresh each app is, or its newest unread
 * notification, and let owners and admins manage access.
 *
 * `aside` (the view switch) sits at the end of the first heading.
 */
export function HomeApps({
  view,
  apps,
  recent,
  orgName,
  onManageAccess,
  aside
}: {
  view: HomeView;
  apps: CustomAppSummary[];
  recent: readonly CustomAppSummary[];
  orgName: string | undefined;
  onManageAccess?: (app: CustomAppSummary) => void;
  aside: ReactNode;
}) {
  const showRecent = view === "simple" && apps.length >= RECENT_FROM && recent.length > 0;
  const heading = (id: string, text: string) => (
    <div className='flex min-h-8 items-center justify-between gap-4'>
      <h2 id={id} className={HEADING}>
        {text}
      </h2>
      {aside}
    </div>
  );
  return (
    <div className='flex flex-col gap-8'>
      {showRecent && (
        <section className='flex flex-col gap-4' aria-labelledby='home-recent-apps-heading'>
          {heading("home-recent-apps-heading", "Recent")}
          <HomeAppIcons apps={recent} testId={(app) => `home-recent-app-${app.slug}`} />
        </section>
      )}
      {/* The container the full grid's columns are measured against (`@container`). */}
      <section className='@container flex flex-col gap-4' aria-labelledby='home-apps-heading'>
        {showRecent ? (
          <h2 id='home-apps-heading' className={HEADING}>
            All apps
          </h2>
        ) : (
          heading("home-apps-heading", "Apps")
        )}
        {apps.length === 0 ? (
          <FirstAppsOnTheirWay orgName={orgName} />
        ) : view === "simple" ? (
          <HomeAppIcons apps={apps} testId={(app) => `launcher-app-card-${app.slug}`} />
        ) : (
          <HomeAppGrid apps={apps} onManageAccess={onManageAccess} large />
        )}
      </section>
    </div>
  );
}
