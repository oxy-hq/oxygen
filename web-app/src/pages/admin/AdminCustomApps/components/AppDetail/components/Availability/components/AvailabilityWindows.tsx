import type { AvailabilityWindow } from "@/types/apps";
import { formatWindow } from "../availabilityTone";

/**
 * Counts per measurement window, shortest first.
 *
 * The short windows say whether something is wrong *now*, the long ones whether
 * it has been wrong for a while — so a row dirty at 24h and clean at 5m is an
 * incident that already ended, and the reverse is one that just started. That
 * comparison is the whole reason the raw windows are shown rather than a single
 * headline number.
 */
export const AvailabilityWindows = ({ windows }: { windows: AvailabilityWindow[] }) => {
  if (windows.length === 0) {
    return (
      <p className='text-muted-foreground text-xs' data-testid='admin-app-availability-empty'>
        No measurement windows returned.
      </p>
    );
  }
  return (
    <div className='overflow-hidden border' data-testid='admin-app-availability-windows'>
      <table className='w-full text-xs'>
        <thead className='bg-muted/50 text-muted-foreground'>
          <tr>
            <th className='px-3 py-1.5 text-left font-medium'>Window</th>
            <th className='px-3 py-1.5 text-right font-medium'>Requests</th>
            <th className='px-3 py-1.5 text-right font-medium'>Failed</th>
            <th className='px-3 py-1.5 text-right font-medium'>Failure rate</th>
          </tr>
        </thead>
        <tbody>
          {windows.map((w) => (
            <tr
              key={w.window_minutes}
              className='border-t'
              data-testid={`admin-app-availability-window-${w.window_minutes}`}
            >
              <td className='px-3 py-1.5 font-mono'>{formatWindow(w.window_minutes)}</td>
              <td className='px-3 py-1.5 text-right tabular-nums'>{w.total.toLocaleString()}</td>
              <td className='px-3 py-1.5 text-right tabular-nums'>{w.failed.toLocaleString()}</td>
              <td className='px-3 py-1.5 text-right tabular-nums'>
                {/* An empty window has NO rate — printing 0% for one would draw
                    a healthy line over silence. */}
                {w.failure_ratio === null ? (
                  <span className='text-muted-foreground'>—</span>
                ) : (
                  `${(w.failure_ratio * 100).toFixed(w.failure_ratio < 0.01 ? 2 : 1)}%`
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
};
