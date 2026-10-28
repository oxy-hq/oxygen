import { TriangleAlert } from "lucide-react";
import { Link } from "react-router-dom";
import { useHqAlerts } from "./useHqAlerts";

const bannerClass =
  "flex items-center gap-3 rounded-lg border border-destructive/40 bg-destructive/10 px-4 py-2.5 transition-colors hover:bg-destructive/15";

/** Top-of-page banner reserved for **critical** signals only (source outage,
 *  store offline, major revenue anomaly). Hidden in the calm default state —
 *  the persistent feed lives in `NeedsAttention` below the cards. Renders
 *  above the app cards when something is genuinely urgent.
 *
 *  Not dismissible yet: a dismiss control should only exist once it's backed
 *  by real notification state. TODO(notifications): add state-backed dismiss. */
export function CriticalAlertBanner() {
  const critical = useHqAlerts().filter((s) => s.severity === "critical");
  if (critical.length === 0) return null;
  return (
    <div className='mx-auto w-full max-w-6xl px-6 pb-4' data-testid='hq-critical-banner'>
      <div className='flex flex-col gap-2'>
        {critical.map((signal) => {
          const content = (
            <>
              <TriangleAlert className='size-4 shrink-0 text-destructive' />
              <span className='min-w-0 flex-1 truncate text-sm'>
                <span className='font-medium text-destructive'>{signal.category}</span>
                <span className='text-foreground/90'> — {signal.title}</span>
              </span>
              <span className='shrink-0 text-muted-foreground text-xs'>{signal.destLabel}</span>
            </>
          );
          // A custom app is a document of its own, so a plain anchor into it
          // (same tab); Oxygen Factory is in-SPA.
          return signal.href ? (
            <a key={signal.id} href={signal.href} className={bannerClass}>
              {content}
            </a>
          ) : (
            <Link key={signal.id} to={signal.route ?? ""} className={bannerClass}>
              {content}
            </Link>
          );
        })}
      </div>
    </div>
  );
}
