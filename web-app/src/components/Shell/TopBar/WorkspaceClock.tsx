import { CalendarDays } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import useWorkspaceTimezone from "@/hooks/api/agents/useWorkspaceTimezone";
import { TodayPanel } from "./TodayPanel";

/**
 * Live clock in the workspace's timezone (default `America/Los_Angeles`), not
 * UTC — derived from the default agent's `.agentic.yml` `timezone:`. Updates
 * each half-minute (minute precision), pausing when the tab is hidden.
 *
 * `withDate` is the menu bar's form ("Thu, Oct 29, 3:42 PM"), the zone moved
 * into a tooltip that names it in full. It is also a button: it opens Today, the
 * person's widgets, as a panel under the bar (the way a Mac shows its widgets from
 * the date). On a phone, where there is no room for the text, a calendar icon
 * stands in for it and the panel is as wide as the screen.
 */
export function WorkspaceClock({ withDate = false }: { withDate?: boolean }) {
  const tz = useWorkspaceTimezone();
  const [now, setNow] = useState(() => new Date());

  useEffect(() => {
    const update = () => setNow(new Date());
    const id = setInterval(() => {
      if (!document.hidden) update();
    }, 30_000);
    // Re-show: tick immediately so the clock isn't up to 30s stale after the
    // tab was backgrounded (interval ticks are skipped while hidden).
    const onVisible = () => {
      if (!document.hidden) update();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      clearInterval(id);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }, []);

  const text = useMemo(() => {
    const date = withDate ? ({ weekday: "short", month: "short", day: "numeric" } as const) : {};
    try {
      return new Intl.DateTimeFormat(undefined, {
        timeZone: tz,
        ...date,
        hour: "numeric",
        minute: "2-digit",
        ...(withDate ? {} : { timeZoneName: "short" as const })
      }).format(now);
    } catch {
      // Invalid IANA name → local time without a zone label rather than crash.
      return new Intl.DateTimeFormat(undefined, {
        ...date,
        hour: "numeric",
        minute: "2-digit"
      }).format(now);
    }
  }, [now, tz, withDate]);

  if (withDate) {
    return <TodayClock text={text} zone={zoneLabel(now, tz)} />;
  }

  return (
    <span
      data-testid='topbar-clock'
      className='hidden font-mono text-muted-foreground text-xs tabular-nums sm:inline'
    >
      {text}
    </span>
  );
}

/** "Pacific Daylight Time · America/Los_Angeles", or the bare name if the zone is unknown. */
function zoneLabel(now: Date, tz: string): string {
  try {
    const long = new Intl.DateTimeFormat(undefined, { timeZone: tz, timeZoneName: "long" })
      .formatToParts(now)
      .find((p) => p.type === "timeZoneName")?.value;
    return long ? `${long} · ${tz}` : tz;
  } catch {
    return tz;
  }
}

/** The menu bar's clock as a button: the time (an icon on a phone), a tooltip with the zone, Today's panel. */
function TodayClock({ text, zone }: { text: string; zone: string }) {
  const [open, setOpen] = useState(false);
  const [tipOpen, setTipOpen] = useState(false);
  return (
    <Popover open={open} onOpenChange={setOpen}>
      {/* The tooltip steps aside while the panel is open: it would sit on top of it. */}
      <Tooltip open={tipOpen && !open} onOpenChange={setTipOpen}>
        <TooltipTrigger asChild>
          <PopoverTrigger asChild>
            <button
              type='button'
              data-testid='topbar-clock'
              aria-label={`Today, ${text}`}
              className='flex h-7 shrink-0 items-center gap-1 whitespace-nowrap rounded-md px-1.5 text-foreground text-xs tabular-nums transition-colors hover:bg-sidebar-accent'
            >
              <CalendarDays className='size-4 sm:hidden' />
              <span className='hidden sm:inline'>{text}</span>
            </button>
          </PopoverTrigger>
        </TooltipTrigger>
        <TooltipContent>{zone}</TooltipContent>
      </Tooltip>
      <PopoverContent
        align='end'
        collisionPadding={0}
        data-testid='topbar-today'
        // Own scroll inside the room under the bar; a phone's panel is the screen's width.
        className='flex max-h-(--radix-popover-content-available-height) w-90 flex-col p-0 max-sm:w-screen max-sm:rounded-t-none max-sm:border-x-0'
      >
        <TodayPanel onNavigate={() => setOpen(false)} />
      </PopoverContent>
    </Popover>
  );
}
