import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { AdminStatusPill } from "@/pages/admin/components/AdminStatusPill";
import type { CustomApp } from "@/types/apps";

export const NOT_FROM_CI_DETAIL =
  "The live build was published by a person, not by trusted-publishing CI. See the availability guidelines: publish with oxyc init-ci --promote.";

/**
 * Marks an app whose live build a person published rather than trusted-publishing CI.
 *
 * An app off the CI path is one nobody republishes when a platform change requires it:
 * in the Sep 2026 incident, a release made customer warehouses read-only to apps unless
 * the manifest declared `customerWarehouseWrites`, and the one app without CI publish
 * stayed broken for a week. Muted rather than warn — nothing is wrong *today*; it is a
 * gap that only bites on the next required republish.
 *
 * Only `person` earns it. `ci` is the path we want, and absent means there is nothing
 * to judge (nothing live, or a build older than the publisher column) — reporting
 * either as "not from CI" would be a guess dressed as a finding.
 */
export const NotFromCiBadge = ({ app }: { app: Pick<CustomApp, "live_published_via"> }) => {
  if (app.live_published_via !== "person") return null;
  return (
    <Tooltip>
      {/* A span, not the pill itself: `asChild` needs a child that takes a ref and
          handlers, and the pill does not forward them. Not a button either — this
          sits inside the row's `<Link>`. */}
      <TooltipTrigger asChild>
        <span className='inline-flex' data-testid='admin-apps-fleet-not-from-ci'>
          <AdminStatusPill tone='muted' label='Not from CI' />
        </span>
      </TooltipTrigger>
      <TooltipContent className='max-w-xs'>{NOT_FROM_CI_DETAIL}</TooltipContent>
    </Tooltip>
  );
};
