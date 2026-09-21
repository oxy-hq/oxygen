import { ArrowRight, HardDrive, type LucideIcon, ShieldCheck } from "lucide-react";
import { Link } from "react-router-dom";
import { useOxyAccessGrants } from "@/hooks/api/customApps/useOxyAccessGrants";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { CustomApp, FleetStorageResponse } from "@/types/apps";
import { formatBytes } from "../StorageTab/utils";

/**
 * The facts that are true of the fleet rather than of any app in it.
 *
 * This exists for two reasons, and the second is the one worth remembering.
 *
 * **It gives the escape hatches a home on the landing.** Storage and staff-access are
 * routed, not tabs, which is right — but in the first cut they were reachable *only*
 * from inside an app's console, so the two genuinely fleet-wide views hid behind an
 * arbitrary app. A link to "storage across every app" belongs on the page that is about
 * every app.
 *
 * **And it answers the one defect this rebuilt fleet would otherwise have reintroduced.**
 * The original brief measured the old landing as "~90% empty space: three rows in a
 * full-width table 1000px tall", and a three-row list is still a three-row list. The
 * cure is not padding or taller rows — stretching three rows to fill a viewport is how
 * the old `AppCard` gallery happened. It is that a fleet page is exactly where fleet
 * totals belong, and they were nowhere on it. So the space under the list carries
 * information that is otherwise unreachable from here, and the page reads as finished
 * because it *is* finished, not because it was inflated.
 *
 * Every number is read from a response or derived from the rows on screen. Nothing here
 * is computed optimistically: while the storage rollup is in flight the storage card
 * says so rather than showing a zero that would read as "this fleet stores nothing".
 */
export const FleetSummary = ({
  apps,
  storage
}: {
  apps: readonly CustomApp[];
  storage: FleetStorageResponse | undefined;
}) => {
  const { data: grants, isError: grantsFailed } = useOxyAccessGrants();
  const locked = grants?.filter((g) => g.locked).length ?? 0;
  const orgs = new Set(apps.map((a) => a.org_slug));
  const published = apps.filter((a) => a.published_at).length;

  return (
    <div
      className='grid shrink-0 gap-3 border-t px-4 py-3 md:grid-cols-2'
      data-testid='admin-apps-fleet-summary'
    >
      <SummaryCard
        icon={HardDrive}
        title='Storage across every app'
        to='/admin/apps/storage'
        testid='admin-apps-fleet-storage-link'
      >
        {storage ? (
          <>
            <Stat value={formatBytes(storage.totalBytes)} label='total' />
            <Stat value={storage.totalObjects.toLocaleString()} label='objects' />
            {storage.unmeasuredApps > 0 && (
              <Stat value={String(storage.unmeasuredApps)} label='never measured' />
            )}
            {storage.totalsAreFloor && (
              <span className={cn("text-xs", ADMIN_TONE.warn.text)}>
                A walk was incomplete — these totals are a floor.
              </span>
            )}
          </>
        ) : (
          // Not "0 bytes". An absent rollup and an empty one are different facts.
          <span className='text-muted-foreground text-xs'>Rollup has not arrived yet.</span>
        )}
      </SummaryCard>

      <SummaryCard
        icon={ShieldCheck}
        title='Oxy staff access'
        to='/admin/apps/access'
        testid='admin-apps-fleet-access-link'
      >
        {/* A card's stats must answer its own heading. This shipped showing org and
            published counts — neither of which is about staff access — so the only
            sentence on it that matched the title was the subtitle. `locked` is the
            whole question: which workspaces have shut Oxy out. */}
        {grants ? (
          locked > 0 ? (
            <>
              <Stat value={String(locked)} label={locked === 1 ? "workspace" : "workspaces"} />
              <span className={cn("text-xs", ADMIN_TONE.warn.text)}>
                have locked Oxy staff out.
              </span>
            </>
          ) : (
            <span className='text-muted-foreground text-xs'>
              No workspace has locked Oxy staff out.
            </span>
          )
        ) : grantsFailed ? (
          // A source that did not answer is a THIRD outcome, not an empty one — the rule
          // `AdminHome`'s reports exist for. "Hasn't arrived yet" would imply it still
          // might, on the one card where a quiet absence reads as an all-clear.
          <span className={cn("text-xs", ADMIN_TONE.warn.text)}>
            Couldn&rsquo;t check who has locked us out.
          </span>
        ) : (
          // Not "0 locked": pending and clean are different facts too.
          <span className='text-muted-foreground text-xs'>Checking&hellip;</span>
        )}
        <Stat value={String(orgs.size)} label={orgs.size === 1 ? "org" : "orgs"} />
        <Stat value={`${published}/${apps.length}`} label='published' />
      </SummaryCard>
    </div>
  );
};

const SummaryCard = ({
  icon: Icon,
  title,
  to,
  testid,
  children
}: {
  icon: LucideIcon;
  title: string;
  to: string;
  testid: string;
  children: React.ReactNode;
}) => (
  <Link
    to={to}
    data-testid={testid}
    className='group flex min-w-0 flex-col gap-2 rounded-md border border-border/60 p-3 transition-colors hover:bg-muted/40'
  >
    <span className='flex items-center gap-1.5'>
      <Icon className='size-3.5 shrink-0 text-muted-foreground' />
      <span className='flex-1 truncate font-semibold text-sm'>{title}</span>
      <ArrowRight className='size-3 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5' />
    </span>
    <span className='flex flex-wrap items-baseline gap-x-4 gap-y-1'>{children}</span>
  </Link>
);

const Stat = ({ value, label }: { value: string; label: string }) => (
  <span className='flex items-baseline gap-1.5'>
    <span className='font-medium text-sm tabular-nums'>{value}</span>
    <span className='text-muted-foreground text-xs'>{label}</span>
  </span>
);
