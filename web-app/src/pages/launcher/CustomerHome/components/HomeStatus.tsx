import { Eye } from "lucide-react";
import { cn } from "@/libs/shadcn/utils";
import type { WidgetCard } from "@/types/homeWidgets";
import type { Since } from "../useSinceLastVisit";
import { needsCount } from "./HomeWidgets";
import { isUnavailable } from "./HomeWidgets/utils";

/** What the day holds, in one line, and the colour of its dot. */
export type TodayStatus = { text: string; dot: "bg-destructive" | "bg-warning" | null };

/**
 * The day in one line: how many widgets have something to do, or that all is
 * clear. Null with no widgets, or when none answered: there is no day to report.
 */
export function todayStatus(widgets: WidgetCard[] | undefined): TodayStatus | null {
  if (!widgets?.length || widgets.every(isUnavailable)) return null;
  const needs = needsCount(widgets);
  const failed = widgets.filter(isUnavailable).length;
  const dot =
    needs === 0
      ? null
      : widgets.some((w) => w.view?.level === "critical")
        ? "bg-destructive"
        : widgets.some((w) => w.view?.level === "attention")
          ? "bg-warning"
          : null;
  const text =
    needs > 0
      ? `${needs} ${needs === 1 ? "thing needs" : "things need"} you today`
      : failed > 0
        ? `All clear, but ${failed} couldn't load`
        : "All clear today";
  return { text, dot };
}

/** The status line's parts: its dot, its words, and what changed since the last visit. */
export function StatusParts({
  status,
  since
}: {
  status: TodayStatus;
  since: ReadonlyMap<string, Since>;
}) {
  return (
    <>
      {status.dot && (
        <span aria-hidden className={cn("size-2 shrink-0 rounded-full", status.dot)} />
      )}
      <span className='truncate'>{status.text}</span>
      {/* What changed while the person was away, so a return visit says why to look. */}
      {since.size > 0 && (
        <>
          <span aria-hidden className='size-1 shrink-0 rounded-full bg-muted-foreground/50' />
          <span data-testid='home-status-since' className='shrink-0'>
            {since.size} {since.size === 1 ? "update" : "updates"}
          </span>
        </>
      )}
    </>
  );
}

/** With Today hidden, the one way back: small and quiet, where the day's status line would be. */
export function ShowToday({ onShow }: { onShow: () => void }) {
  return (
    <button
      type='button'
      data-testid='home-today-show'
      onClick={onShow}
      className='flex min-h-6 pointer-coarse:min-h-11 w-fit items-center gap-1.5 text-muted-foreground text-sm transition-colors hover:text-foreground'
    >
      <Eye className='size-3.5' />
      Show Today
    </button>
  );
}
