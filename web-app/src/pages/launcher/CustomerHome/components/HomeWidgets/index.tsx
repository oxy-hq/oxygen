import { Button } from "@/components/ui/shadcn/button";
import { useHomeWidgets } from "@/hooks/api/homeWidgets/useHomeWidgets";
import { cn } from "@/libs/shadcn/utils";
import type { CustomAppSummary } from "@/types/apps";
import { HOME_GAP } from "../../layout";
import { WidgetCard } from "./components/WidgetCard";

/**
 * Each app's live cards: what needs doing in it now, with the button that does
 * it. Nothing at all while no app declares a widget, so a home without them is
 * the home as it was. Design: `internal-docs/home-widgets.md`.
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
  return (
    <section
      className='flex flex-col gap-3'
      aria-label={title ?? "Widgets"}
      data-testid='home-widgets'
      data-group={title ?? undefined}
    >
      {title && <h2 className='font-medium text-base'>{title}</h2>}
      <div className={cn("grid sm:grid-cols-2 lg:grid-cols-3", HOME_GAP)}>
        {cards.map((card) => (
          <WidgetCard
            key={card.key}
            card={card}
            orgSlug={orgSlug}
            iconUrl={icons.get(card.app.id)}
          />
        ))}
      </div>
    </section>
  );
}
