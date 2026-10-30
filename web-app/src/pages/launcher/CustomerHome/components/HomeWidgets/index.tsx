import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { useHomeWidgets } from "@/hooks/api/homeWidgets/useHomeWidgets";
import { cn } from "@/libs/shadcn/utils";
import type { CustomAppSummary } from "@/types/apps";
import { HOME_GAP } from "../../layout";
import { WidgetCard } from "./components/WidgetCard";
import { WidgetStrip } from "./components/WidgetStrip";
import { isCalm, widgetTestId } from "./utils";

/**
 * Each app's live cards: what needs doing in it now, with the button that does
 * it. Nothing at all while no app declares a widget, so a home without them is
 * the home as it was. Design: `internal-docs/home-widgets.md`.
 *
 * Room follows relevance: a widget with something to do is a full card, first;
 * a calm one (`isCalm`) folds to a line after them. Both keep the group's order
 * among themselves. A card someone pressed stays whole for the visit, so the
 * answer to the press is not folded away when the view turns calm.
 */
export function HomeWidgets({
  orgSlug,
  apps,
  group
}: {
  orgSlug: string | undefined;
  /** The home's app list, for each widget's app icon. */
  apps: CustomAppSummary[];
  /** The layout's group this draws: its title, and its copies' keys in order.
   *  Without one (no layout loaded yet), every copy, untitled. */
  group?: { title: string | null; items: string[] };
}) {
  const { data: widgets, isError, refetch, isFetching } = useHomeWidgets(orgSlug);
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
  if (!orgSlug || !widgets?.length) return null;
  const byKey = new Map(widgets.map((w) => [w.key, w]));
  const cards = group ? group.items.flatMap((key) => byKey.get(key) ?? []) : widgets;
  if (!cards.length) return null;
  const icons = new Map(apps.map((a) => [a.id, a.icon_url]));
  const title = group?.title;
  const folded = (card: (typeof cards)[number]) => isCalm(card) && !pressed.has(card.key);
  const full = cards.filter((card) => !folded(card));
  const strips = cards.filter(folded);
  return (
    <section
      className='flex flex-col gap-3'
      aria-label={title ?? "Widgets"}
      data-testid='home-widgets'
      data-group={title ?? undefined}
    >
      {title && <h2 className='font-medium text-base'>{title}</h2>}
      {full.length > 0 && (
        <div className={cn("grid sm:grid-cols-2 lg:grid-cols-3", HOME_GAP)}>
          {full.map((card) => (
            <WidgetCard
              key={card.key}
              card={card}
              orgSlug={orgSlug}
              iconUrl={icons.get(card.app.id)}
              onPressed={() => setPressed((was) => new Set(was).add(card.key))}
            />
          ))}
        </div>
      )}
      {strips.length > 0 && (
        // The cards' column gap, so a line sits under the card beside it; rows of lines sit closer.
        <div
          className='grid gap-x-4 gap-y-2 sm:grid-cols-2 lg:grid-cols-3'
          data-testid='home-widgets-calm'
        >
          {strips.map((card) => (
            <WidgetStrip
              key={card.key}
              card={card}
              iconUrl={icons.get(card.app.id)}
              testId={widgetTestId(card)}
            />
          ))}
        </div>
      )}
    </section>
  );
}
