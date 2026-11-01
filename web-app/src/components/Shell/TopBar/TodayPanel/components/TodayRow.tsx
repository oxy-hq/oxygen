import { AppMark } from "@/components/apps/AppMark";
import { cn } from "@/libs/shadcn/utils";
import { LEVEL_DOT } from "@/pages/launcher/CustomerHome/components/HomeWidgets/utils";
import type { WidgetCard } from "@/types/homeWidgets";
import { rowTestId } from "../utils";

/**
 * One widget copy, compact: the app's icon, its title, the headline, and, when
 * `count` is set and the view has progress, the count ("85/97"). A count, not a ring: at this size an empty
 * ring reads as a radio button. The whole row opens the view's link, or the
 * app, in this tab like every other way into one.
 */
export function TodayRow({
  card,
  iconUrl,
  count = false
}: {
  card: WidgetCard;
  iconUrl?: string | null;
  count?: boolean;
}) {
  const view = card.view;
  const dot = view ? LEVEL_DOT[view.level] : null;
  const headline = view ? view.headline : "Couldn't load";
  const testId = rowTestId(card);
  return (
    <a
      href={view?.href ?? card.app.url}
      data-testid={testId}
      data-level={view?.level}
      aria-label={`${card.app.name} · ${card.title}: ${headline}`}
      // A failed widget's reason, for whoever owns the app; the row itself stays calm.
      title={view ? undefined : (card.error ?? undefined)}
      className='flex min-h-11 min-w-0 items-center gap-3 rounded-md px-2 py-1.5 text-sm transition-colors hover:bg-muted'
    >
      <AppMark iconUrl={iconUrl} name={card.app.name} size='sm' />
      <span className='flex min-w-0 flex-1 flex-col'>
        <span className='truncate font-medium text-foreground'>{card.title}</span>
        <span className='flex min-w-0 items-center gap-1.5 text-muted-foreground text-xs'>
          {dot && <span aria-hidden className={cn("size-1.5 shrink-0 rounded-full", dot)} />}
          <span className='truncate' data-testid={`${testId}-headline`}>
            {headline}
          </span>
        </span>
      </span>
      {count && view?.progress && (
        <span
          className='shrink-0 text-muted-foreground text-xs tabular-nums'
          data-testid={`${testId}-progress`}
        >
          {view.progress.done}/{view.progress.total}
        </span>
      )}
    </a>
  );
}
