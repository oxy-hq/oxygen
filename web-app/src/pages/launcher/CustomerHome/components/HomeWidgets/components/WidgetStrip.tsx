import { ArrowRight } from "lucide-react";
import { AppMark } from "@/components/apps/AppMark";
import { cn } from "@/libs/shadcn/utils";
import type { WidgetCard as Card } from "@/types/homeWidgets";
import { LEVEL_DOT } from "../utils";

/**
 * A calm widget, folded to one line: its title and headline, opening the app.
 * The full card is for a widget with something to do; one that only says all
 * is well takes the room of a line, the way a phone's stack gives the screen
 * to the widget that matters now.
 */
export function WidgetStrip({
  card,
  iconUrl,
  testId
}: {
  card: Card;
  iconUrl?: string | null;
  testId: string;
}) {
  const view = card.view;
  const dot = view ? LEVEL_DOT[view.level] : null;
  return (
    <a
      href={view?.href ?? card.app.url}
      data-testid={testId}
      data-level={view?.level}
      data-compact='true'
      aria-label={`${card.app.name} · ${card.title}: ${view?.headline ?? ""}`}
      className='group flex min-h-11 min-w-0 items-center gap-2 rounded-lg border bg-card px-4 py-2.5 text-sm hover:bg-accent'
    >
      <AppMark iconUrl={iconUrl} name={card.app.name} size='sm' />
      <span className='shrink-0 font-medium text-card-foreground'>{card.title}</span>
      <span
        aria-hidden
        className={cn("size-1.5 shrink-0 rounded-full", dot ?? "bg-muted-foreground")}
      />
      <span className='min-w-0 truncate text-muted-foreground' data-testid={`${testId}-headline`}>
        {view?.headline}
      </span>
      <ArrowRight className='ml-auto size-3.5 shrink-0 text-muted-foreground group-hover:text-foreground' />
    </a>
  );
}
