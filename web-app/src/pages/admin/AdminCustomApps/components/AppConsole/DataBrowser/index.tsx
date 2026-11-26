import { Database, Lock } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import type { CustomApp } from "@/types/apps";
import { ConsolePanel } from "../../ConsolePanel";
import { DataBrowser } from "./DataBrowser";

/**
 * "Data": a read-only look at what the app has stored with `ctx.oltp`, in the
 * environment the console's switch names — production's schema, or staging's or a
 * sandbox's own where it has one.
 *
 * **Nothing here writes, and nothing loads on its own.** The server reads in a
 * read-only transaction from the app's own schema, and writes an audit row for every
 * read that reaches the store, so a page load must not be what does it: opening the
 * console would put a row in the customer's audit log and wake a suspended database
 * for someone who came to look at a build. "Browse data" is the act; after it the
 * panel follows the switch.
 *
 * The lock line is the storage browser's, worded for a browser that has no write mode
 * to enable.
 */
export const AppDataPanel = ({
  app,
  environment,
  scope
}: {
  app: CustomApp;
  environment: string;
  /** The console's environment chip, as the other panels that follow the switch. */
  scope: { label: string; warn?: boolean };
}) => {
  const [browsing, setBrowsing] = useState(false);

  return (
    <ConsolePanel
      id='data'
      title='Data'
      scope={scope}
      info={`The tables ${app.name} stores with ctx.oltp in ${scope.label.toLowerCase()}, a page at a time. Read-only: nothing here can change the app's data, and every read is recorded in the audit log.`}
    >
      <div className='flex min-w-0 flex-col gap-2' data-testid='admin-app-data'>
        <p
          className='flex items-center gap-1.5 text-muted-foreground text-xs'
          data-testid='admin-app-data-mode'
        >
          <Lock className='size-3 shrink-0' />
          Read-only · every read is audited
        </p>
        {browsing ? (
          // Keyed by environment: a table and a page of staging's mean nothing in
          // production's schema, so switching starts over at that schema's table list.
          <DataBrowser key={environment} appId={app.id} environment={environment} />
        ) : (
          <div
            className='flex flex-wrap items-center justify-between gap-2 border border-border/60 border-dashed px-3 py-2'
            data-testid='admin-app-data-gate'
          >
            <span className='font-mono text-muted-foreground text-xs'>ctx.oltp</span>
            <Button
              variant='outline'
              size='sm'
              className='h-7 gap-1.5 text-xs'
              onClick={() => setBrowsing(true)}
              data-testid='admin-app-data-browse'
            >
              <Database className='size-3' />
              Browse data
            </Button>
          </div>
        )}
      </div>
    </ConsolePanel>
  );
};
