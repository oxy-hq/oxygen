import { ChevronDown } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger
} from "@/components/ui/shadcn/collapsible";
import type { WidgetCard } from "@/types/homeWidgets";
import type { HomeView } from "../useHomeView";
import type { Since } from "../useSinceLastVisit";
import { StatusParts, todayStatus } from "./HomeStatus";

const key = (view: HomeView) => `oxy.home.today-open.${view}`;

/** Folded on the simple home, open on the full one, until the person says otherwise. */
function readOpen(view: HomeView): boolean {
  try {
    const stored = window.localStorage.getItem(key(view));
    return stored === null ? view === "full" : stored === "1";
  } catch {
    return view === "full";
  }
}

/**
 * Today under the apps, as one line that opens to the widgets: the day's status,
 * with a red or amber dot when something needs someone and how many widgets
 * changed since the last visit. Each view remembers whether the person keeps it
 * open, in this browser. Nothing while there is no Today: no widgets, none hidden,
 * and no failed read.
 */
export function TodayFold({
  view,
  widgets,
  hiddenCount,
  failed,
  since,
  children
}: {
  view: HomeView;
  widgets: WidgetCard[] | undefined;
  /** Copies the person hid: Today stays, to bring them back. */
  hiddenCount: number;
  /** The widgets read failed: Today opens to its retry. */
  failed: boolean;
  since: ReadonlyMap<string, Since>;
  /** Today itself, drawn when the fold is open. */
  children: ReactNode;
}) {
  const [open, setOpen] = useState(() => readOpen(view));
  useEffect(() => setOpen(readOpen(view)), [view]);
  if (!failed && !widgets?.length && hiddenCount === 0) return null;
  const status = todayStatus(widgets) ?? {
    text: failed ? "Today couldn't load" : "Today",
    dot: null
  };
  const toggle = (next: boolean) => {
    setOpen(next);
    try {
      window.localStorage.setItem(key(view), next ? "1" : "0");
    } catch {
      // Private mode or full storage: the fold starts as the view's default next time.
    }
  };
  return (
    <Collapsible open={open} onOpenChange={toggle} data-testid='home-today-fold'>
      <CollapsibleTrigger
        data-testid='home-today-toggle'
        className='group flex min-h-8 pointer-coarse:min-h-11 min-w-0 max-w-full items-center gap-2 rounded-md font-medium text-muted-foreground text-sm transition-colors hover:text-foreground'
      >
        <StatusParts status={status} since={since} />
        <ChevronDown
          aria-hidden
          className='size-4 shrink-0 transition-transform duration-(--duration-fast) group-data-[state=open]:rotate-180 motion-reduce:transition-none'
        />
      </CollapsibleTrigger>
      <CollapsibleContent className='pt-4'>{children}</CollapsibleContent>
    </Collapsible>
  );
}
