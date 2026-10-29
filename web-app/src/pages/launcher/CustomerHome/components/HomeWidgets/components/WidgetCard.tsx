import { ArrowUpRight } from "lucide-react";
import { useState } from "react";
import { AppMark } from "@/components/apps/AppMark";
import { usePressWidget } from "@/hooks/api/homeWidgets/usePressWidget";
import { cn } from "@/libs/shadcn/utils";
import type { WidgetCard as Card, WidgetButton as WidgetButtonData } from "@/types/homeWidgets";
import { LEVEL_DOT, pressError, ROW_STATE } from "../utils";
import { WidgetButton } from "./WidgetButton";

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
  iconUrl
}: {
  card: Card;
  orgSlug: string;
  iconUrl?: string | null;
}) {
  const press = usePressWidget(orgSlug);
  const [said, setSaid] = useState<Said>(null);
  const [pressing, setPressing] = useState<WidgetButtonData | null>(null);
  const view = card.view;
  const base = `home-widget-${card.app.slug}-${card.id}`;

  const run = (button: WidgetButtonData) => {
    setSaid(null);
    setPressing(button);
    press.mutate(
      { appId: card.app.id, widget: card.id, button },
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

  return (
    <article
      data-testid={base}
      data-level={view?.level}
      className={cn(
        "flex flex-col gap-3 rounded-lg border bg-card p-4",
        card.size === "wide" && "sm:col-span-2"
      )}
    >
      <header className='flex items-center gap-2 text-muted-foreground text-xs'>
        <AppMark iconUrl={iconUrl} name={card.app.name} size='sm' />
        <span className='min-w-0 truncate'>
          {card.app.name} · {card.title}
        </span>
        <a
          href={view?.href ?? card.app.url}
          data-testid={`${base}-open`}
          className='ml-auto flex shrink-0 items-center gap-0.5 hover:text-foreground'
        >
          Open
          <ArrowUpRight className='size-3.5' />
        </a>
      </header>

      {view ? (
        <>
          <div className='flex flex-col gap-0.5'>
            <p className='flex items-center gap-2 font-medium text-base text-card-foreground'>
              {dot && <span aria-hidden className={cn("size-2 shrink-0 rounded-full", dot)} />}
              <span data-testid={`${base}-headline`}>{view.headline}</span>
            </p>
            {view.detail && <p className='text-muted-foreground text-sm'>{view.detail}</p>}
          </div>

          {view.progress && (
            <div
              role='progressbar'
              aria-valuemin={0}
              aria-valuemax={view.progress.total}
              aria-valuenow={view.progress.done}
              className='h-1.5 overflow-hidden rounded-full bg-muted'
              data-testid={`${base}-progress`}
            >
              <div
                className='h-full rounded-full bg-primary'
                style={{ width: `${(view.progress.done / view.progress.total) * 100}%` }}
              />
            </div>
          )}

          {view.rows.length > 0 && (
            <ul className='flex flex-col divide-y text-sm'>
              {view.rows.map((row, i) => {
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
