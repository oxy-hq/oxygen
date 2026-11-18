import { ChevronRight, ShieldAlert } from "lucide-react";
import type React from "react";
import { useMemo } from "react";
import { Link } from "react-router-dom";
import useCameras from "@/hooks/api/cameras/useCameras";
import { useSelectedSite } from "@/hooks/useSelectedSite";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * "Setup needed" prompt for cameras that exist in the workspace but
 * haven't been reviewed yet (their `analytics_consent` is still
 * `false`). Camera creation defaults to consent-off as a
 * privacy-safe starting point; this banner is the surface that
 * keeps the operator from accidentally leaving a freshly imported
 * site permanently silent.
 *
 * Hidden when no cameras are pending review so the page stays
 * clean. Click-through routes to the Devices > Cameras tab in
 * `?review=1` mode, which filters the cameras table to the
 * unreviewed set and surfaces the bulk-enable action.
 *
 * Scoped by the global SitePicker — the count and the link
 * inherit the site filter so banner action is always consistent
 * with what the operator is looking at.
 */
const SetupNeededBanner: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const { siteId: selectedSite } = useSelectedSite();
  const { data: cameras = [] } = useCameras(workspace?.id);

  const pending = useMemo(() => {
    const list = selectedSite ? cameras.filter((c) => c.site_id === selectedSite) : cameras;
    return list.filter((c) => c.active && !c.analytics_consent);
  }, [cameras, selectedSite]);

  if (pending.length === 0) return null;

  const devicesUrl = ROUTES.ORG(orgSlug).WORKSPACE().IDE.EDGE.DEVICES;
  // Preserve the active site in the deep-link so the destination
  // table is filtered to the same scope the banner counted. Without
  // this, "3 cameras awaiting review" in Site A would land on a
  // table that lists *all* unreviewed cameras across every site —
  // and the bulk-enable action would flip consent outside the
  // banner's scope.
  const reviewParams = new URLSearchParams({ review: "1" });
  if (selectedSite) reviewParams.set("site", selectedSite);
  const reviewUrl = `${devicesUrl}?${reviewParams.toString()}`;
  const noun = pending.length === 1 ? "camera" : "cameras";

  return (
    <div
      className='flex items-center justify-between gap-3 rounded-md border border-amber-500/40 bg-amber-500/5 px-4 py-2 text-amber-900 dark:text-amber-200'
      data-testid='setup-needed-banner'
    >
      <div className='flex min-w-0 items-center gap-2'>
        <ShieldAlert className='h-4 w-4 shrink-0' />
        <div className='flex min-w-0 flex-col text-xs'>
          <span className='font-medium'>
            {pending.length} {noun} awaiting review
          </span>
          <span className='text-amber-800/80 dark:text-amber-200/70'>
            Camera analytics is off by default. Confirm consent and set zones to start producing
            compliance reports.
          </span>
        </div>
      </div>
      <Link
        to={reviewUrl}
        className='inline-flex shrink-0 items-center gap-1 rounded-md border border-amber-500/50 bg-amber-500/10 px-2.5 py-1 font-medium text-xs hover:bg-amber-500/20'
      >
        Review
        <ChevronRight className='h-3.5 w-3.5' />
      </Link>
    </div>
  );
};

export default SetupNeededBanner;
