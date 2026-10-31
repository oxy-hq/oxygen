import { ArrowRight } from "lucide-react";
import { useState } from "react";
import { AppMark } from "@/components/apps/AppMark";
import { usePressWidget } from "@/hooks/api/homeWidgets/usePressWidget";
import { cn } from "@/libs/shadcn/utils";
import type { WidgetCard as Card, WidgetButton as WidgetButtonData } from "@/types/homeWidgets";
import { LEVEL_DOT, pressError, ROW_STATE, widgetTestId } from "../utils";
import { WidgetButton } from "./WidgetButton";
import { WidgetMetric } from "./WidgetMetric";
import { WidgetProgress } from "./WidgetProgress";

/**
 * Rows a card shows. Every card is one column wide (the Today rail, or a card in
 * a stacked column), so a size says how much a card shows, not how wide it is: a
 * `regular` card is short, a `wide` or `full` one shows every row the view sent.
 */
const ROWS_SHOWN: Record<Card["size"], number> = { regular: 3, wide: 8, full: 8 };

/** What the last press said, under the buttons. */
type Said = { kind: "message" | "error"; text: string } | null;

/**
 * One app's widget: its headline, progress, a few rows and up to two buttons,
 * drawn by the home from the view the app's function returned. No app code runs
 * here. "Open" goes into the app, in this tab, like every other way into it.
 */
export function WidgetCard({
  card,
  orgSlug,
  iconUrl,
  onPressed
}: {
  card: Card;
  orgSlug: string;
  iconUrl?: string | null;
  /** Told when a button is pressed, so the home keeps this card whole while its answer shows. */
  onPressed?: () => void;
}) {
  const press = usePressWidget(orgSlug);
  const [said, setSaid] = useState<Said>(null);
  const [pressing, setPressing] = useState<WidgetButtonData | null>(null);
  const view = card.view;
  const base = widgetTestId(card);

  const run = (button: WidgetButtonData) => {
    setSaid(null);
    setPressing(button);
    onPressed?.();
    press.mutate(
      { appId: card.app.id, widget: card.id, key: card.key, button },
      {
        onSuccess: (answer) =>
          setSaid(answer.message ? { kind: "message", text: answer.message } : null),
        onError: (error) => setSaid({ kind: "error", text: pressError(error) }),
        onSettled: () => setPressing(null)
      }
    );
  };
  const buttonProps = (button: WidgetButtonData, testId: string, secondary?: boolean) => ({
    button,
    testId,
    secondary,
    pending: pressing === button,
    disabled: press.isPending,
    onPress: () => run(button)
  });
  const dot = view ? LEVEL_DOT[view.level] : null;
  const allRows = view?.rows ?? [];
  // A row with a button is work to do on the home: a card that has one keeps every row.
  const rows = allRows.some((row) => row.button)
    ? allRows
    : allRows.slice(0, ROWS_SHOWN[card.size]);
  const moreRows = allRows.length - rows.length;

  return (
    <article
      data-testid={base}
      data-level={view?.level}
      data-size={card.size}
      className='flex min-w-0 starting:translate-y-1.5 flex-col gap-3 rounded-lg border bg-card p-4 starting:opacity-0 transition-[opacity,translate] duration-(--duration-slow) motion-reduce:starting:translate-y-0'
    >
      <header className='flex items-center gap-2 text-muted-foreground text-xs'>
        <AppMark iconUrl={iconUrl} name={card.app.name} size='sm' />
        <span className='min-w-0 truncate'>
          {card.app.name} · {card.title}
        </span>
        <a
          href={view?.href ?? card.app.url}
          data-testid={`${base}-open`}
          // Every card has an "Open"; a screen reader's list of links has to tell them apart.
          aria-label={`Open ${card.app.name}: ${card.title}`}
          className='-my-1 pointer-coarse:-my-3 ml-auto flex min-h-6 pointer-coarse:min-h-11 shrink-0 items-center gap-0.5 hover:text-foreground'
        >
          Open
          <ArrowRight className='size-3.5' />
        </a>
      </header>

      {view ? (
        <>
          <div className='flex items-center gap-3'>
            <div className='flex min-w-0 flex-1 flex-col gap-0.5'>
              <p className='flex items-center gap-2 font-medium text-base text-card-foreground'>
                {dot && <span aria-hidden className={cn("size-2 shrink-0 rounded-full", dot)} />}
                <span data-testid={`${base}-headline`}>{view.headline}</span>
              </p>
              {view.detail && <p className='text-muted-foreground text-sm'>{view.detail}</p>}
            </div>
            {/* "N of M" as a ring beside the headline: a card is one column, with no width for a bar. */}
            {view.progress && <WidgetProgress {...view.progress} testId={`${base}-progress`} />}
          </div>

          {view.metric && <WidgetMetric metric={view.metric} testId={`${base}-metric`} />}

          {rows.length > 0 && (
            <ul className='flex flex-col divide-y text-sm'>
              {rows.map((row, i) => {
                const state = row.state ? ROW_STATE[row.state] : null;
                const title = row.href ? (
                  <a href={row.href} className='hover:underline'>
                    {row.title}
                  </a>
                ) : (
                  row.title
                );
                return (
                  <li
                    // biome-ignore lint/suspicious/noArrayIndexKey: rows have no id of their own; their order is the app's
                    key={`${i}-${row.title}`}
                    data-testid={`${base}-row-${i}`}
                    className='flex items-center gap-3 py-2'
                  >
                    <div className='flex min-w-0 flex-1 flex-col'>
                      <span className='truncate text-card-foreground'>{title}</span>
                      {row.detail && (
                        <span className='truncate text-muted-foreground text-xs'>{row.detail}</span>
                      )}
                    </div>
                    {state && (
                      <span className={cn("shrink-0 text-xs", state.className)}>{state.label}</span>
                    )}
                    {row.button && (
                      <WidgetButton {...buttonProps(row.button, `${base}-row-${i}-button`, true)} />
                    )}
                  </li>
                );
              })}
            </ul>
          )}
          {moreRows > 0 && (
            // Says what the cap left out, as `oxyc home show` does, and where to see it.
            <a
              href={view.href ?? card.app.url}
              data-testid={`${base}-more`}
              className='-mt-2 w-fit text-muted-foreground text-xs hover:text-foreground'
            >
              {moreRows} more in {card.app.name}
            </a>
          )}

          {view.buttons.length > 0 && (
            <div className='mt-auto flex flex-wrap items-center gap-2'>
              {view.buttons.map((button, i) => (
                <WidgetButton
                  key={button.action + (button.key ?? i)}
                  {...buttonProps(button, `${base}-button-${button.action}`, i > 0)}
                />
              ))}
            </div>
          )}
        </>
      ) : (
        <p className='text-muted-foreground text-sm' data-testid={`${base}-error`}>
          {card.error ?? "This widget returned nothing to show."}
        </p>
      )}

      {said && (
        <p
          role={said.kind === "error" ? "alert" : "status"}
          data-testid={`${base}-${said.kind}`}
          className={cn(
            "text-xs",
            said.kind === "error" ? "text-destructive" : "text-muted-foreground"
          )}
        >
          {said.text}
        </p>
      )}
    </article>
  );
}
