import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import useOrgCustomApps from "@/hooks/api/customApps/useOrgCustomApps";
import { useHomeWidgets } from "@/hooks/api/homeWidgets/useHomeWidgets";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import type { WidgetCard } from "@/types/homeWidgets";
import { TodayRow } from "./components/TodayRow";
import { groupForPanel } from "./utils";

/**
 * Today from the top bar: the person's widget copies in a compact list, from any
 * page, so what needs them is one click away without opening the home. It reads
 * the home's own request (same query key, so opening it costs nothing when the home
 * has just loaded), and shows Today whatever the person chose for their home: a
 * copy they hid stays hidden, because the server does not send it.
 *
 * Needs-you first, then calm, then "Couldn't load". Mounted only while open, so
 * nothing is fetched until someone looks.
 */
export function TodayPanel({ onNavigate }: { onNavigate: () => void }) {
  const orgSlug = useCurrentOrg((s) => s.org?.slug);
  const workspaceId = useCurrentWorkspace((s) => s.workspace?.id) ?? "";
  const { apps } = useOrgCustomApps(workspaceId);
  const { data: widgets, isError, isPending, refetch, isFetching } = useHomeWidgets(orgSlug);
  const icons = new Map(apps.map((a) => [a.id, a.icon_url]));

  return (
    <div className='flex min-h-0 flex-1 flex-col'>
      <h2 className='px-3 pt-3 pb-1 font-medium text-sm'>Today</h2>
      <div className='min-h-0 flex-1 overflow-y-auto px-1 pb-1' data-testid='topbar-today-body'>
        {isError ? (
          <p
            className='flex items-center gap-2 px-2 py-3 text-muted-foreground text-sm'
            data-testid='topbar-today-error'
          >
            Couldn't load Today.
            <Button
              variant='link'
              size='sm'
              className='h-auto p-0'
              disabled={isFetching}
              onClick={() => refetch()}
              data-testid='topbar-today-retry'
            >
              Retry
            </Button>
          </p>
        ) : isPending ? (
          <PanelSkeleton />
        ) : (
          <Rows widgets={widgets} icons={icons} />
        )}
      </div>
      <div className='border-t px-1 py-1'>
        <Button
          asChild
          variant='ghost'
          size='sm'
          className='pointer-coarse:min-h-11 w-full justify-start text-muted-foreground'
        >
          <Link
            to={ROUTES.ORG(orgSlug ?? "").HOME}
            onClick={onNavigate}
            data-testid='topbar-today-home'
          >
            Open home
          </Link>
        </Button>
      </div>
    </div>
  );
}

function Rows({
  widgets,
  icons
}: {
  widgets: WidgetCard[];
  icons: Map<string, string | null | undefined>;
}) {
  if (widgets.length === 0) {
    return (
      <p className='px-2 py-3 text-muted-foreground text-sm' data-testid='topbar-today-empty'>
        Nothing for you today.
      </p>
    );
  }
  const { needs, calm, failed } = groupForPanel(widgets);
  // Counts only where someone has a job: "0/17" beside "All checklists on time" is noise.
  const group = (cards: WidgetCard[], testId: string, heading?: string, counts = false) =>
    cards.length > 0 && (
      <section className='flex flex-col' data-testid={testId}>
        {heading && <h3 className='px-2 pt-2 pb-1 text-muted-foreground text-xs'>{heading}</h3>}
        {cards.map((card) => (
          <TodayRow key={card.key} card={card} iconUrl={icons.get(card.app.id)} count={counts} />
        ))}
      </section>
    );
  return (
    <>
      {group(needs, "topbar-today-needs", undefined, true)}
      {group(calm, "topbar-today-calm", "All clear")}
      {group(failed, "topbar-today-failed", "Couldn't load")}
    </>
  );
}

function PanelSkeleton() {
  return (
    <div className='flex flex-col gap-2 p-2' aria-busy='true' data-testid='topbar-today-loading'>
      <Skeleton className='h-9 w-full' />
      <Skeleton className='h-9 w-full' />
      <Skeleton className='h-9 w-full' />
    </div>
  );
}
