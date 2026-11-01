import { ArrowRight } from "lucide-react";
import { AppMark } from "@/components/apps/AppMark";
import { cn } from "@/libs/shadcn/utils";
import type { WidgetCard as Card } from "@/types/homeWidgets";
import { LEVEL_DOT } from "../utils";
import { EXIT_CLASSES, useExit } from "./useExit";
import { WidgetMenu } from "./WidgetMenu";

/**
 * A calm widget, folded to one line: its title and headline, opening the app.
 * The full card is for a widget with something to do; one that only says all
 * is well takes the room of a line, the way a phone's stack gives the screen
 * to the widget that matters now.
 */
export function WidgetStrip({
  card,
  iconUrl,
  testId,
  onHide
}: {
  card: Card;
  iconUrl?: string | null;
  testId: string;
  /** Hides this copy from the person's home. Called once the line's exit has played. */
  onHide?: () => void;
}) {
  const exit = useExit(() => onHide?.());
  const view = card.view;
  const dot = view ? LEVEL_DOT[view.level] : null;
  return (
    // The line is one link; its menu sits beside it (a button cannot live inside a link).
    <div
      data-testid={`${testId}-line`}
      {...exit.exitProps}
      className={cn(
        "group/widget relative flex min-w-0 starting:translate-y-1 starting:opacity-0 transition-[opacity,translate] duration-(--duration-slow) motion-reduce:starting:translate-y-0",
        EXIT_CLASSES
      )}
    >
      <a
        href={view?.href ?? card.app.url}
        data-testid={testId}
        data-level={view?.level}
        data-compact='true'
        aria-label={`${card.app.name} · ${card.title}: ${view?.headline ?? "Couldn't load"}`}
        // A failed widget's reason, for whoever owns the app; the line itself stays calm.
        title={view ? undefined : (card.error ?? undefined)}
        className='group flex min-h-11 min-w-0 flex-1 items-center gap-2 rounded-lg border bg-card px-4 py-2.5 text-sm transition-[background-color] hover:bg-accent'
      >
        <AppMark iconUrl={iconUrl} name={card.app.name} size='sm' />
        <span className='shrink-0 font-medium text-card-foreground'>{card.title}</span>
        <span
          aria-hidden
          className={cn("size-1.5 shrink-0 rounded-full", dot ?? "bg-muted-foreground")}
        />
        <span className='min-w-0 truncate text-muted-foreground' data-testid={`${testId}-headline`}>
          {view ? view.headline : "Couldn't load"}
        </span>
        {/* Where the menu appears, so the two never overlap. */}
        <ArrowRight
          className={cn(
            "ml-auto size-3.5 shrink-0 text-muted-foreground group-hover:text-foreground",
            onHide &&
              "pointer-coarse:opacity-0 group-focus-within/widget:opacity-0 group-hover/widget:opacity-0 group-has-data-[state=open]/widget:opacity-0"
          )}
        />
      </a>
      {onHide && (
        <WidgetMenu
          testId={`${testId}-menu`}
          label={`${card.app.name}: ${card.title}`}
          onHide={exit.leave}
          className='absolute top-1/2 right-2 -translate-y-1/2'
        />
      )}
    </div>
  );
}
