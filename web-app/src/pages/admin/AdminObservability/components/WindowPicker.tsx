import { cn } from "@/libs/shadcn/utils";

/**
 * The window every readout on this surface shares.
 *
 * 30 days is the default, and the reason is the question: a cost page is asked
 * "what will this cost us", and a single day is noise. Vercel's usage page
 * defaults to 30 for the same reason, and its own guidance is to read a
 * 30-day window to spot a pattern.
 *
 * 90 is here because a monthly cycle needs more than one month to show itself;
 * 7 because an incident this week is not visible under a 30-day average.
 */
export const WINDOWS = [7, 30, 90] as const;
export type WindowDays = (typeof WINDOWS)[number];

export const DEFAULT_WINDOW: WindowDays = 30;

/** Is `n` one of the offered windows? Used to read `?days=` back off the URL. */
export function isWindow(n: number): n is WindowDays {
  return (WINDOWS as readonly number[]).includes(n);
}

export const WindowPicker = ({
  days,
  onChange
}: {
  days: number;
  onChange: (days: WindowDays) => void;
}) => (
  <div
    className='flex items-center gap-1'
    role='group'
    aria-label='Window'
    data-testid='admin-observability-window'
  >
    {WINDOWS.map((w) => (
      <button
        key={w}
        type='button'
        onClick={() => onChange(w)}
        aria-pressed={days === w}
        data-testid={`admin-observability-window-${w}`}
        className={cn(
          " px-2 py-1 font-medium text-[11px] uppercase tracking-wide transition-colors",
          days === w
            ? "bg-foreground text-background"
            : "text-muted-foreground hover:bg-muted hover:text-foreground"
        )}
      >
        {w}d
      </button>
    ))}
  </div>
);
