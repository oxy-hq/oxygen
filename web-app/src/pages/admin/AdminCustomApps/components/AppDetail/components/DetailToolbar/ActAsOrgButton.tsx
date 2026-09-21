import { ShieldAlert } from "lucide-react";
import { useRef, useState } from "react";
import { AssumeRoleDialog } from "@/components/admin/AssumeRoleDialog";
import { Button } from "@/components/ui/shadcn/button";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/shadcn/tooltip";
import { useActingSession } from "@/hooks/api/adminAssume/useActingSession";
import { cn } from "@/libs/shadcn/utils";
import { clearAssumeDestination, rememberAssumeDestination } from "@/libs/utils/assumeDestination";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { CustomApp } from "@/types/apps";

/**
 * "Open this app as the customer" — the one action that turns an admin preview
 * into the real thing.
 *
 * The preview iframe on this page renders the customer's bundle under *your*
 * staff identity, so every data call it makes is refused: staff hold no
 * membership in a tenant org, and the data plane says so
 * (`x-oxy-assume-required`). The app looks broken, and the fix — assume the org's
 * role — used to mean leaving for the org list, finding the org, assuming it, and
 * then navigating back to the app by hand.
 *
 * Assuming a role also closes the entire admin surface
 * (`assume::block_admin_while_acting`, per **user** — not per tab, so "keep admin
 * open in the other tab" cannot work). That guard is deliberate and untouched
 * here. What's fixed is the trip around it: one click assumes the org, lands
 * directly on the app with real data, and stopping brings you back to this exact
 * page.
 */
export function ActAsOrgButton({ app }: { app: CustomApp }) {
  const [open, setOpen] = useState(false);
  const { isActing } = useActingSession();
  // Set by `onStarted`, which the dialog fires immediately *after* it closes.
  // See the microtask in `onOpenChange`.
  const started = useRef(false);

  // The subpath URL, not `url_subdomain`: a custom-app subdomain is a separate
  // origin with its own session cookie, and this navigation is same-origin.
  const landing = appPath(app);
  const returnTo = `/admin/apps/${app.org_slug}/${app.slug}`;

  const openDialog = () => {
    // Recorded before the session exists, because both legs of the trip survive
    // only in storage: starting and stopping are full page loads.
    rememberAssumeDestination({ orgId: app.org_id, landing, returnTo });
    setOpen(true);
  };

  return (
    <>
      <Tooltip>
        <TooltipTrigger asChild>
          {/* `span` wrapper: a disabled button fires no pointer events, so Radix
              would never see the hover that explains WHY it's disabled. */}
          <span>
            <Button
              variant='outline'
              size='sm'
              className='h-6 gap-1 px-1.5 text-[11px]'
              disabled={isActing}
              onClick={openDialog}
              data-testid='admin-app-act-as-org'
            >
              <ShieldAlert className={cn("size-3", ADMIN_TONE.warn.text)} />
              Act as org
            </Button>
          </span>
        </TooltipTrigger>
        <TooltipContent className='max-w-xs space-y-1.5'>
          {isActing ? (
            <p>You're already acting as an organization. Stop that session first.</p>
          ) : (
            <>
              <p className='font-medium'>Open this app as {app.org_slug}</p>
              <p>
                Starts an audited session as this organization and opens the app with the customer's
                real data — the preview above runs as you, so its queries are refused.
              </p>
              <p className='text-muted-foreground'>
                60 minutes, and it can't be extended. Admin closes while you're acting; stopping
                brings you back to this page.
              </p>
            </>
          )}
        </TooltipContent>
      </Tooltip>

      <AssumeRoleDialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (next) return;
          // Closed. On success the dialog closes and *then* calls `onStarted`, so
          // the check is deferred by one microtask — clearing synchronously here
          // would wipe the trip home out from under a session that just started
          // (`window.location.assign` unloads the page asynchronously).
          queueMicrotask(() => {
            if (!started.current) clearAssumeDestination();
          });
        }}
        org={{ id: app.org_id, name: app.org_slug }}
        onStarted={() => {
          started.current = true;
        }}
      />
    </>
  );
}

/**
 * The app's same-origin path, `/customer-apps/<org_slug>/<app_slug>/` — a
 * preserved wire contract. Taken from the server's canonical `url` so a future
 * change to that shape carries over, with the literal as the fallback.
 */
function appPath(app: CustomApp): string {
  const fallback = `/customer-apps/${app.org_slug}/${app.slug}/`;
  try {
    const url = new URL(app.url, window.location.origin);
    // `app.url` is the canonical same-origin subpath. If it ever resolved
    // off-origin (an absolute subdomain URL), stripping it to `pathname+search`
    // would silently land on the CURRENT origin's path instead of the app —
    // fall back to the known subpath rather than navigate somewhere stray.
    if (url.origin !== window.location.origin) return fallback;
    return `${url.pathname}${url.search}`;
  } catch {
    return fallback;
  }
}
