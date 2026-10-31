import { ChevronRight } from "lucide-react";
import { Link } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import { type AlertSeverity, type HqSignal, useHqAlerts } from "./useHqAlerts";

const dotClass: Record<AlertSeverity, string> = {
  critical: "bg-destructive",
  warning: "bg-amber-500",
  info: "bg-muted-foreground/60"
};

const rowClass = "group flex items-center gap-3 rounded px-1 py-1.5 hover:bg-muted/40";

function SignalRow({ signal }: { signal: HqSignal }) {
  const content = (
    <>
      <span className={cn("h-2 w-2 shrink-0 rounded-full", dotClass[signal.severity])} />
      <span className='min-w-0 flex-1 truncate text-sm'>
        <span className='font-medium text-foreground/90'>{signal.category}</span>
        <span className='text-muted-foreground'> — {signal.title}</span>
      </span>
      <span className='shrink-0 text-muted-foreground text-xs'>{signal.destLabel}</span>
      <ChevronRight className='size-3.5 shrink-0 text-muted-foreground/50 group-hover:text-muted-foreground' />
    </>
  );
  // A custom app is a document of its own, so a plain anchor into it (same
  // tab); Oxygen Factory is in-SPA.
  return signal.href ? (
    <a href={signal.href} data-testid={`hq-signal-${signal.id}`} className={rowClass}>
      {content}
    </a>
  ) : (
    <Link to={signal.route ?? ""} data-testid={`hq-signal-${signal.id}`} className={rowClass}>
      {content}
    </Link>
  );
}

/** HQ "Needs attention" — a calm, secondary intelligence module below the app
 *  cards. Surfaces the persistent signals Oxygen is monitoring; truly
 *  critical/urgent items surface separately via CriticalAlertBanner above the
 *  cards.
 *
 *  TODO(notifications): when a real notification system lands these become
 *  dismissible/stateful — do NOT add fake dismiss controls before then. */
export function NeedsAttention({
  includeUnmeasured = true,
  contained = true
}: {
  /** Whether "Not measured" rows (`info`: no health signal, not a problem with the
   *  app) are listed. The apps-first home lists them only for Factory users, who
   *  can do something about them. */
  includeUnmeasured?: boolean;
  /** Whether it brings the page's own container. The apps-first home draws it
   *  inside the apps' column, which already sits in that container. */
  contained?: boolean;
} = {}) {
  const signals = useHqAlerts().filter(
    (s) => s.severity !== "critical" && (includeUnmeasured || s.severity !== "info")
  );
  if (signals.length === 0) return null;
  return (
    <div
      className={cn("w-full pb-8", contained && "mx-auto max-w-6xl px-6")}
      data-testid='hq-needs-attention'
    >
      <p className='mb-1.5 font-medium text-muted-foreground/70 text-xs uppercase tracking-wide'>
        Needs attention
      </p>
      <div className='flex flex-col'>
        {signals.map((signal) => (
          <SignalRow key={signal.id} signal={signal} />
        ))}
      </div>
    </div>
  );
}
