import { useEffect, useMemo, useState } from "react";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import useWorkspaceTimezone from "@/hooks/api/agents/useWorkspaceTimezone";

/**
 * Live clock in the workspace's timezone (default `America/Los_Angeles`), not
 * UTC — derived from the default agent's `.agentic.yml` `timezone:`. Updates
 * each half-minute (minute precision), pausing when the tab is hidden.
 *
 * `withDate` is the menu bar's form ("Thu, Oct 29, 3:42 PM"), the zone moved
 * into a tooltip that names it in full.
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
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            data-testid='topbar-clock'
            className='hidden whitespace-nowrap px-1 text-foreground text-xs tabular-nums sm:inline'
          >
            {text}
          </span>
        </TooltipTrigger>
        <TooltipContent>{zoneLabel(now, tz)}</TooltipContent>
      </Tooltip>
    );
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
