import { useState } from "react";
import { ToggleGroup, ToggleGroupItem } from "@/components/ui/shadcn/toggle-group";
import { ClientErrors } from "./components/ClientErrors";
import { FunctionLogs } from "./components/FunctionLogs";
import { DEFAULT_LOG_WINDOW, LOG_WINDOWS, type LogWindowHours } from "./functionLogs";

/**
 * Logs — what the app actually did, from both sides of the wire.
 *
 * Server side is persisted `ctx.log()` / `console.*` from Oxy Functions; before
 * these were stored, a route call's output lived only inside the HTTP response
 * that carried it and vanished the moment the caller navigated away. Client
 * side is uncaught browser errors with **message and stack** — the platform
 * used to keep names and counts only, which made a white-screened app report
 * `{TypeError: 3}` and nothing anyone could act on.
 *
 * Stacks are source-mapped on the server. The maps are no longer served to
 * browsers, so this panel is the only place they are applied.
 *
 * One window governs both lists: "what did the browser throw" and "what did the
 * server print" are asked about the same stretch of time.
 */
export const AppLogs = ({ orgSlug, appSlug }: { orgSlug: string; appSlug: string }) => {
  const [hours, setHours] = useState<LogWindowHours>(DEFAULT_LOG_WINDOW);

  return (
    <div className='space-y-4 p-4 pt-0' data-testid='admin-app-logs'>
      <div className='flex items-center justify-between gap-2'>
        <p className='text-muted-foreground text-xs'>Times are UTC.</p>
        <ToggleGroup
          type='single'
          size='sm'
          variant='outline'
          value={String(hours)}
          onValueChange={(next) => {
            const picked = LOG_WINDOWS.find((w) => String(w.hours) === next);
            if (picked) setHours(picked.hours);
          }}
          aria-label='Log window'
          data-testid='admin-app-logs-window'
        >
          {LOG_WINDOWS.map((w) => (
            <ToggleGroupItem key={w.hours} value={String(w.hours)} className='h-6 px-2 text-xs'>
              {w.label}
            </ToggleGroupItem>
          ))}
        </ToggleGroup>
      </div>
      <section className='space-y-2'>
        <h3 className='font-semibold text-sm'>Client errors</h3>
        <ClientErrors orgSlug={orgSlug} appSlug={appSlug} hours={hours} />
      </section>
      <section className='space-y-2'>
        <h3 className='font-semibold text-sm'>Function output</h3>
        <FunctionLogs orgSlug={orgSlug} appSlug={appSlug} hours={hours} />
      </section>
    </div>
  );
};
