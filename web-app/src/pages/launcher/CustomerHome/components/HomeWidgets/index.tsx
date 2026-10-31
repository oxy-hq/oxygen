import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useHomeWidgets } from "@/hooks/api/homeWidgets/useHomeWidgets";
import { cn } from "@/libs/shadcn/utils";
import type { WidgetCard as Card } from "@/types/homeWidgets";
import { WidgetCard } from "./components/WidgetCard";
import { WidgetStrip } from "./components/WidgetStrip";
import { isCalm, isUnavailable, widgetTestId } from "./utils";

/** A widgets group of the org's layout: its title, and its copies' keys in order. */
export type WidgetGroup = { title: string | null; items: string[] };

/** The anchor the home's status line jumps to. */
export const TODAY_ANCHOR = "home-today";

/**
 * Today: every widget, in a space of its own. Nothing at all while no app
 * declares a widget. Design: `internal-docs/home-widgets.md`.
 *
 * What needs someone comes first, everywhere: the widgets with something to do
 * are full cards under their layout group's heading, then every calm one
 * (`isCalm`) is a line in one "All clear" list, then any widget that failed to
 * answer is a quiet "Couldn't load" line: an app's timeout is its owner's news,
 * not the loudest thing on everyone's home. `rail` is the desktop column
 * beside the apps; `stack` is the same order under the apps on a phone or a
 * tablet, so nothing sits off-screen behind a swipe.
 *
 * A card someone pressed stays whole for the visit, so the answer to the press is
 * not folded away when the view turns calm.
 */
export function HomeWidgets({
  orgSlug,
  apps,
  groups,
  variant
}: {
  orgSlug: string | undefined;
  /** The home's app list, for each widget's app icon. */
  apps: { id: string; icon_url?: string | null }[];
  /** The layout's widgets groups, in order. Empty before a layout loads: every copy, untitled. */
  groups: WidgetGroup[];
  variant: "rail" | "stack";
}) {
  const { data: widgets, isError, isPending, refetch, isFetching } = useHomeWidgets(orgSlug);
  const [pressed, setPressed] = useState<ReadonlySet<string>>(new Set());
  if (isError) {
    return (
      <p
        className='flex items-center gap-2 text-muted-foreground text-sm'
        data-testid='home-widgets-error'
      >
        Widgets couldn't load.
        <Button
          variant='link'
          size='sm'
          className='h-auto p-0'
          disabled={isFetching}
          onClick={() => refetch()}
          data-testid='home-widgets-retry'
        >
          Retry
        </Button>
      </p>
    );
  }
  if (orgSlug && isPending) return <TodaySkeleton />;
  if (!orgSlug || !widgets?.length) return null;

  const byKey = new Map(widgets.map((w) => [w.key, w]));
  const placed = groups.length
    ? groups.map((g) => ({ title: g.title, cards: g.items.flatMap((key) => byKey.get(key) ?? []) }))
    : [{ title: null, cards: widgets }];
  const folded = (card: Card) => isCalm(card) && !pressed.has(card.key);
  const full = (card: Card) => !folded(card) && !isUnavailable(card);
  const loud = placed
    .map((g) => ({ title: g.title, cards: g.cards.filter(full) }))
    .filter((g) => g.cards.length > 0);
  const calm = placed.flatMap((g) => g.cards.filter(folded));
  const failed = placed.flatMap((g) => g.cards.filter(isUnavailable));
  if (!loud.length && !calm.length && !failed.length) return null;

  const icons = new Map(apps.map((a) => [a.id, a.icon_url]));
  // One column that may shrink (`grid-cols-1` is `minmax(0, 1fr)`), so a card's
  // longest line cannot push it past its column. Under the apps on a tablet the
  // calm lines sit two to a row; a card keeps the full width.
  const cardColumn = "grid grid-cols-1 gap-3";
  const lineColumns = cn("grid grid-cols-1 gap-2", variant === "stack" && "sm:grid-cols-2");

  return (
    <section
      id={TODAY_ANCHOR}
      className='flex scroll-mt-6 flex-col gap-6'
      aria-labelledby='home-today-heading'
      data-testid='home-widgets'
      data-variant={variant}
    >
      <h2 id='home-today-heading' className='font-medium text-base'>
        Today
      </h2>
      {loud.map((group, i) => (
        <div
          // Groups have no id of their own; their order is the layout's.
          // biome-ignore lint/suspicious/noArrayIndexKey: see above
          key={i}
          className='flex flex-col gap-3'
          data-testid='home-widgets-group'
          data-group={group.title ?? undefined}
        >
          {group.title && (
            <h3 className='font-medium text-muted-foreground text-sm'>{group.title}</h3>
          )}
          <div className={cardColumn}>
            {group.cards.map((card) => (
              <WidgetCard
                key={card.key}
                card={card}
                orgSlug={orgSlug}
                iconUrl={icons.get(card.app.id)}
                onPressed={() => setPressed((was) => new Set(was).add(card.key))}
              />
            ))}
          </div>
        </div>
      ))}
      {calm.length > 0 && (
        <div className='flex flex-col gap-3' data-testid='home-widgets-calm'>
          <h3 className='font-medium text-muted-foreground text-sm'>All clear</h3>
          <div className={lineColumns}>
            {calm.map((card) => (
              <WidgetStrip
                key={card.key}
                card={card}
                iconUrl={icons.get(card.app.id)}
                testId={widgetTestId(card)}
              />
            ))}
          </div>
        </div>
      )}
      {failed.length > 0 && (
        <div className='flex flex-col gap-3' data-testid='home-widgets-failed'>
          <h3 className='font-medium text-muted-foreground text-sm'>Couldn't load</h3>
          <div className={lineColumns}>
            {failed.map((card) => (
              <WidgetStrip
                key={card.key}
                card={card}
                iconUrl={icons.get(card.app.id)}
                testId={widgetTestId(card)}
              />
            ))}
          </div>
        </div>
      )}
    </section>
  );
}

/** How many widgets have something to do: what the home's status line counts. A
 *  widget that failed to answer is not a job for the person reading the home. */
export function needsCount(widgets: Card[] | undefined): number {
  return (widgets ?? []).filter((card) => !isCalm(card) && !isUnavailable(card)).length;
}

/** Today's shape while the widgets load, where this browser knows there is a Today. */
function TodaySkeleton() {
  return (
    <div className='flex flex-col gap-3' aria-busy='true' data-testid='home-widgets-loading'>
      <Skeleton className='h-5 w-16' />
      <Skeleton className='h-32 w-full rounded-lg' />
      <Skeleton className='h-11 w-full rounded-lg' />
      <Skeleton className='h-11 w-full rounded-lg' />
    </div>
  );
}
