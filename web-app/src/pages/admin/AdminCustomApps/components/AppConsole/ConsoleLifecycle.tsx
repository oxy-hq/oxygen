import type { CustomApp } from "@/types/apps";
import { AppLifecycle } from "../AppLifecycle";
import { BuildChecksSummary } from "./BuildChecks";
import { checksLink } from "./checksParams";
import { consolePagePath } from "./consolePages";

/**
 * The lifecycle card as the console shows it: with a Checks row for the build staging
 * serves, and production's smoke linking to its evidence. Both open the Releases
 * page's Checks panel on that build; `search` is the address those links carry over.
 */
export const ConsoleLifecycle = ({ app, search = "" }: { app: CustomApp; search?: string }) => (
  <AppLifecycle
    app={app}
    stagingChecks={(build) => (
      <BuildChecksSummary
        appId={app.id}
        build={build}
        to={consolePagePath(app, "releases", search, checksLink(build.build_id))}
        promotionChecks={app.promotion_checks}
      />
    )}
    smokeTo={(live) =>
      consolePagePath(app, "releases", search, checksLink(live.build_id, "production"))
    }
  />
);
