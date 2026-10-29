import { TriangleAlert } from "lucide-react";
import { Link } from "react-router-dom";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/shadcn/popover";
import { cn } from "@/libs/shadcn/utils";
import { type HqSignal, useHqAlerts } from "@/pages/launcher/components/useHqAlerts";

const rowClass =
  "flex min-w-0 flex-col gap-0.5 rounded-md px-2 py-1.5 text-sm transition-colors hover:bg-muted";

function SignalRow({ signal }: { signal: HqSignal }) {
  const content = (
    <>
      <span
        className={cn(
          "font-medium text-xs",
          signal.severity === "critical" ? "text-destructive" : "text-warning"
        )}
      >
        {signal.category}
      </span>
      <span className='truncate text-foreground'>{signal.title}</span>
    </>
  );
  // A custom app is a document of its own (same-tab anchor); the Factory is in-SPA.
  return signal.href ? (
    <a href={signal.href} className={rowClass} data-testid={`menubar-health-row-${signal.id}`}>
      {content}
    </a>
  ) : (
    <Link
      to={signal.route ?? ""}
      className={rowClass}
      data-testid={`menubar-health-row-${signal.id}`}
    >
      {content}
    </Link>
  );
}

/**
 * The menu bar's app-health item: apps that are down or degraded, from the same
 * verdicts the home's alert banner reads. Nothing at all while there is no such
 * signal — an absence is not a claim that every app is healthy, so no green tick.
 * "Not measured" stays on the home's attention list; it is a gap, not an outage.
 */
export function AppHealthItem() {
  const signals = useHqAlerts().filter(
    (s) => s.severity === "critical" || s.severity === "warning"
  );
  if (signals.length === 0) return null;
  const critical = signals.some((s) => s.severity === "critical");
  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type='button'
          data-testid='menubar-health'
          aria-label={`${signals.length} app${signals.length === 1 ? "" : "s"} need attention`}
          className={cn(
            "flex h-7 items-center gap-1 rounded-md px-1.5 font-medium text-xs tabular-nums transition-colors hover:bg-sidebar-accent",
            critical ? "text-destructive" : "text-warning"
          )}
        >
          <TriangleAlert className='size-4' />
          {signals.length}
        </button>
      </PopoverTrigger>
      <PopoverContent align='end' className='w-72 p-1' data-testid='menubar-health-popover'>
        {signals.map((signal) => (
          <SignalRow key={signal.id} signal={signal} />
        ))}
      </PopoverContent>
    </Popover>
  );
}
