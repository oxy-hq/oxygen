import { cn } from "@/libs/shadcn/utils";
import type { CustomAppSummary } from "@/types/apps";
import { APP_GRID_COLUMNS, type AppRow, HOME_GAP } from "../layout";
import { HomeAppCard } from "./HomeAppCard";

/** How many leading cards load their art at once: the first row at the widest layout. */
const EAGER_ART = 4;

const CARD_SIZE = { one: "wide", two: "regular", many: "regular", dense: "compact" } as const;

/** The home's apps, on the columns `layout.ts` gives for how many there are. */
export function HomeAppGrid({
  apps,
  row,
  onManageAccess
}: {
  apps: CustomAppSummary[];
  row: Exclude<AppRow, "none">;
  onManageAccess?: (app: CustomAppSummary) => void;
}) {
  return (
    <div
      className={cn("grid", HOME_GAP, APP_GRID_COLUMNS[row])}
      data-testid='launcher-app-grid'
      data-layout={row}
    >
      {apps.map((app, i) => (
        // art_url in the key remounts the card when art changes, resetting its error fallback
        <HomeAppCard
          key={`${app.id}:${app.art_url ?? ""}`}
          app={app}
          size={CARD_SIZE[row]}
          eagerArt={i < EAGER_ART}
          onManageAccess={onManageAccess}
        />
      ))}
    </div>
  );
}
