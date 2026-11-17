import { Skeleton } from "@/components/ui/shadcn/skeleton";
import {
  AVAILABILITY_REREAD_SECONDS,
  useListedAvailability
} from "@/hooks/api/customApps/useCustomApps";
import { timeAgo } from "@/libs/utils/date";
import type { CustomApp } from "@/types/apps";
import { AvailabilityVerdict } from "./components/AvailabilityVerdict";
import { AvailabilityWindows } from "./components/AvailabilityWindows";
import { OpenBuildsLine } from "./components/OpenBuildsLine";

/** The slice of an app the panel reads: who it is, and the reading its row carries. */
type AvailabilityApp = Pick<CustomApp, "id" | "org_slug" | "slug" | "availability">;

/**
 * Availability — the *serving* answer, as opposed to the deployment-integrity
 * ladder behind the status badge above it.
 *
 * A custom-app host answers 200 with the SPA shell for every path, so probing
 * an app from outside is green whatever the app is doing. Everything here is
 * derived instead from the requests real people made, which Oxy already
 * terminates: the success ratio over several windows, plus a multi-window
 * error-budget burn verdict.
 *
 * Three verdicts, and the third is not a flavour of healthy — see
 * `availabilityTone`.
 *
 * The reading is the one the app's registry row carries, re-read with that row every
 * minute; there is no availability request of its own. A row without one (a draft, a
 * deployment that does not measure availability, a store that did not answer) shows that
 * there is no reading, which says nothing about whether the app is up.
 */
export const Availability = ({ app }: { app: AvailabilityApp }) => {
  const { reading: data, isLoading, readAt } = useListedAvailability(app);

  if (isLoading) {
    return (
      <div className='space-y-3 p-4 pt-0'>
        <Skeleton className='h-14 w-full' />
        <Skeleton className='h-32 w-full' />
      </div>
    );
  }

  if (!data) {
    return (
      <p
        className='px-4 pb-4 text-muted-foreground text-xs leading-relaxed'
        data-testid='admin-app-availability-none'
      >
        No availability reading. The app may not be published, this deployment may not measure
        availability, or the measurement could not be read. This says nothing about whether the app
        is up.
      </p>
    );
  }

  return (
    <div className='space-y-3 p-4 pt-0' data-testid='admin-app-availability'>
      <AvailabilityVerdict data={data} />
      <AvailabilityWindows windows={data.windows} />
      <OpenBuildsLine open={data.open_builds} />
      {/* A reading's age, next to it: re-read on a timer, so a number that has not
          moved is either a steady app or a re-read that has not happened yet. */}
      <p className='text-[11px] text-muted-foreground' data-testid='admin-app-availability-read'>
        {readAt ? `Read ${timeAgo(new Date(readAt).toISOString())} · ` : ""}re-read every{" "}
        {AVAILABILITY_REREAD_SECONDS === 60 ? "minute" : `${AVAILABILITY_REREAD_SECONDS} seconds`}{" "}
        from real requests.
      </p>
    </div>
  );
};
