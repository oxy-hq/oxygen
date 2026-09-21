import { AppWindow, ArrowLeft } from "lucide-react";
import { Link, useParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { AllowListError, isAllowListError } from "./AllowListError";
import { AppDetail } from "./components/AppDetail";
import { useAdminAppRegistry } from "./useAdminAppRegistry";

/**
 * The third escape hatch: **the full interactive preview**, on its own route.
 *
 * The console demotes the preview to a card, and that is the direction's biggest call —
 * a staff console for running a fleet should not hand its whole stage to an iframe of
 * one customer's app. But "not the landing" is not "deleted", and the distinction
 * matters more here than anywhere else on this surface, because the stage carries a
 * capability nothing else replaces: `useOxyRequestLog` captures the API calls the
 * previewed app makes, so the debug drawer is the only place in the product that answers
 * *what is this app actually calling, and what came back*. Device frames and the
 * draft/published channel toggle ride along.
 *
 * This route exists because the redesign briefly took it out without noticing.
 * `AppDetail` became unreachable, and 2,431 lines of it — `LivePreview`, `DetailToolbar`,
 * the debug panel and their tests — were left alive only by their own test files: dead
 * code that still runs in CI and still reads as maintained. The `PreviewCard` doc comment
 * meanwhile promised the preview was "a tool reached from here rather than the page you
 * land on", which was not true of any code. Routing it is what makes that sentence true.
 *
 * Reached from the console's Preview panel, and never a tab — the same rule the storage
 * and access audits follow, for the same reason: a tab beside the app is how this surface
 * grew four of them.
 */
export default function AppPreviewStage() {
  const params = useParams<{ orgSlug: string; appSlug: string }>();
  const { apps, selected, isLoading, isWalking, error, refetch } = useAdminAppRegistry(
    params.orgSlug,
    params.appSlug
  );
  const backToConsole = `/admin/apps/${params.orgSlug}/${params.appSlug}`;

  return (
    <div className='flex h-[calc(100vh-3.5rem)] flex-col' data-testid='apps-preview-stage'>
      <header className='flex h-10 shrink-0 items-center gap-2 border-b px-3'>
        <Button asChild variant='ghost' size='sm' className='h-7 gap-1.5 text-xs'>
          <Link to={backToConsole} data-testid='apps-preview-stage-back'>
            <ArrowLeft className='size-3.5' />
            Back to console
          </Link>
        </Button>
        {selected && (
          <span className='min-w-0 truncate font-mono text-muted-foreground text-xs'>
            {selected.org_slug}/{selected.slug}
          </span>
        )}
      </header>

      <div className='min-h-0 flex-1'>
        {/* Same rule as the console: a 403 names the fix, everything else goes
            through the kit. */}
        {error && isAllowListError(error) ? (
          <AllowListError error={error} onRetry={refetch} noun='the custom-app registry' />
        ) : (
          <AdminAsync
            // `isWalking` as well as `isLoading`: the latter covers only the FIRST page,
            // so without it an app on page 2 of a >100-app registry renders the stale-link
            // state below — the same confident false claim the console was just fixed for.
            //
            // Note this gates the WHOLE page, where the console renders as soon as
            // `selected` resolves and shows a skeleton only for the not-yet-found case.
            // Deliberate, not a divergence to reconcile: this surface's payload is an
            // iframe that will not paint for far longer than one more page of registry,
            // so holding the skeleton costs nothing here and keeps the branch trivially
            // correct. On the console, where the panels are the content, that wait would
            // be felt.
            query={{
              // `&& !error` for the same reason as the console: `AdminAsync` checks
              // loading before error, and an unconditional `isWalking` here made ANY
              // next-page failure a permanent skeleton with no Retry, not just a deep
              // link to a later-page app.
              isPending: isLoading || (isWalking && !error),
              isError: Boolean(error),
              data: apps,
              error,
              refetch
            }}
            noun='the custom-app registry'
            rows={3}
            className='p-6'
          >
            {() =>
              selected ? (
                <AppDetail app={selected} />
              ) : (
                // Same rule the console follows: a stale link says it is stale rather
                // than bouncing somewhere that looks like it worked.
                <AdminEmptyState
                  className='m-6'
                  icon={AppWindow}
                  title={`No app at ${params.orgSlug}/${params.appSlug}.`}
                  description='It may have been deleted, or the slug may have changed.'
                  action={
                    <Button asChild size='sm' variant='outline' className='h-7 text-xs'>
                      <Link to='/admin/apps'>Back to apps</Link>
                    </Button>
                  }
                />
              )
            }
          </AdminAsync>
        )}
      </div>
    </div>
  );
}
