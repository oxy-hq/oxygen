import { Switch } from "@/components/ui/shadcn/switch";
import { cn } from "@/libs/shadcn/utils";
import type { NotificationChannel, NotificationLevel, SettingsReach } from "@/types/notifications";
import { CHANNELS, type PrefRow, rowNote } from "../utils";
import { FloorMenu } from "./FloorMenu";

/** Indent per depth, on the spacing scale: the source, a category, a kind below it, … */
const INDENT = ["pl-4", "pl-8", "pl-12", "pl-16", "pl-20"];

interface PrefRowViewProps {
  row: PrefRow;
  /** The source's name, for each control's accessible name. */
  source: string;
  /** Names the row's test ids: an app's slug, or `oxygen`. */
  testKey: string;
  reach: SettingsReach;
  /** `true` / `false` set the channel here; `null` goes back to inheriting. */
  onChannel: (channel: NotificationChannel, on: boolean | null) => void;
  onFloor: (level: NotificationLevel | null) => void;
}

/**
 * One row of a source's settings: a Push / Email / Slack switch each, for the
 * channels something under the row is sent on, and its level floor behind
 * "…".
 *
 * Every switch shows what the row gets. One the row inherits says from where,
 * and can be flipped to set the row's own; one set here offers "Reset". A
 * critical row is locked on; a kind under a floor above its level is inbox
 * only, its switches off. Email cannot move for an account with no address.
 */
export function PrefRowView({ row, source, testKey, reach, onChannel, onFloor }: PrefRowViewProps) {
  const note = rowNote(row);
  const name = row.depth === 0 ? source : `${source}, ${row.label}`;
  const testId = `notifications-pref-${testKey}-${row.key}`;

  return (
    <div
      className={cn(
        "flex items-center gap-2 border-b py-2.5 pr-2 last:border-b-0",
        INDENT[Math.min(row.depth, INDENT.length - 1)],
        row.depth === 0 && "bg-muted/30"
      )}
      data-testid={`${testId}-row`}
    >
      <span className='flex min-w-0 flex-1 flex-col'>
        <span className={cn("truncate text-sm", row.depth <= 1 && "font-medium")}>{row.label}</span>
        {row.description && (
          <span className='truncate text-muted-foreground text-xs'>{row.description}</span>
        )}
        {note && (
          <span className='text-muted-foreground text-xs' data-testid={`${testId}-note`}>
            {note}
          </span>
        )}
      </span>
      {CHANNELS.map((c) => {
        const state = row.state[c.value];
        if (!state) {
          return (
            <span key={c.value} className='flex w-12 justify-center sm:w-14'>
              <span
                className='text-muted-foreground text-xs'
                data-testid={`${testId}-${c.value}-none`}
              >
                <span aria-hidden>—</span>
                <span className='sr-only'>{`${name}: not sent by ${c.label}`}</span>
              </span>
            </span>
          );
        }
        const from = !row.critical && !row.inboxOnly && state.from;
        return (
          <span key={c.value} className='flex w-12 flex-col items-center gap-0.5 sm:w-14'>
            <Switch
              checked={row.critical || (state.on && !row.inboxOnly)}
              disabled={row.critical || !!row.inboxOnly || (c.value === "email" && !reach.email)}
              aria-label={from ? `${name}: ${c.label}, from ${from}` : `${name}: ${c.label}`}
              data-testid={`${testId}-${c.value}`}
              onCheckedChange={(on) => onChannel(c.value, on)}
            />
            {state.explicit && !row.critical && (
              <button
                type='button'
                // `relative`: 2px under the switch, it must win its own area from the
                // switch's tap margin.
                className='relative rounded-sm text-muted-foreground text-xs hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring'
                aria-label={`${name}: reset ${c.label}`}
                data-testid={`${testId}-${c.value}-reset`}
                onClick={() => onChannel(c.value, null)}
              >
                Reset
              </button>
            )}
          </span>
        );
      })}
      <span className='flex w-7 justify-center'>
        {!row.critical && <FloorMenu row={row} name={name} testId={testId} onFloor={onFloor} />}
      </span>
    </div>
  );
}
