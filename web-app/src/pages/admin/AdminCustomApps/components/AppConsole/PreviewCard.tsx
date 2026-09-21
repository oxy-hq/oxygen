import { ExternalLink, Monitor } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import type { CustomApp } from "@/types/apps";
import { ConsolePanel } from "../ConsolePanel";

/**
 * The preview, demoted to a card.
 *
 * This is the visible shape of the direction's biggest call. The old detail gave a live
 * iframe of the customer's app **the entire stage** — it was the largest thing on screen —
 * and docked everything an operator came for into a side panel of collapsed accordions.
 * That inverted the surface's job: this is a staff console for running a fleet, not a
 * browser for viewing one app.
 *
 * So the preview is one panel among nine, and it ranks last in the right-hand column.
 * No iframe is rendered here: an embed pulls a full bundle, runs its JS, and re-becomes
 * the biggest thing on the page the moment anyone gives it height.
 *
 * **Demoted, not deleted — and the difference is a route, not a sentence.** An earlier
 * draft of this file claimed the full preview was "a tool reached from here" while
 * nothing in the app reached it, which left `AppDetail` and its 2,431 lines of
 * `LivePreview` / `DetailToolbar` alive only by their own tests. Both buttons below are
 * therefore load-bearing:
 *
 *   - **Inspect** opens the stage at `…/preview` — device frames, the draft/published
 *     channel toggle, and the request log, which is the only place in the product that
 *     shows what a custom app calls and what came back. That is a debugging tool, so it
 *     lives one click away rather than on the page you land on.
 *   - **Open** is the app itself, in a tab, where looking at an app belongs.
 */
export const PreviewCard = ({ app, servedAt }: { app: CustomApp; servedAt: string }) => (
  <ConsolePanel
    id='preview'
    title='Preview'
    question='Q1 — what it is actually calling'
    actions={
      <>
        <Button asChild variant='ghost' size='sm' className='h-6 gap-1 px-1.5 text-xs'>
          <Link
            to={`/admin/apps/${app.org_slug}/${app.slug}/preview`}
            data-testid='apps-console-preview-inspect'
          >
            <Monitor className='size-3' />
            Inspect
          </Link>
        </Button>
        <Button asChild variant='ghost' size='sm' className='h-6 gap-1 px-1.5 text-xs'>
          <a
            href={servedAt}
            target='_blank'
            rel='noreferrer'
            data-testid='apps-console-preview-open'
          >
            Open
            <ExternalLink className='size-3' />
          </a>
        </Button>
      </>
    }
  >
    <div className='flex items-center gap-3'>
      <div className='flex size-9 shrink-0 items-center justify-center rounded-md border border-border/60 bg-muted/40 text-muted-foreground'>
        <Monitor className='size-4' />
      </div>
      <div className='min-w-0 flex-1'>
        <p className='truncate font-mono text-[11px] text-muted-foreground' title={servedAt}>
          {servedAt}
        </p>
        <p className='text-muted-foreground text-xs'>
          Inspect for device sizes, draft channel and the request log.
        </p>
      </div>
    </div>
    <p className='sr-only'>
      Preview for {app.name} in {app.org_slug}
    </p>
  </ConsolePanel>
);
