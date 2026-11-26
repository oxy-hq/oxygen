import { AlertCircle } from "lucide-react";
import { useEffect } from "react";
import { useParams, useSearchParams } from "react-router-dom";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Skeleton } from "@/components/ui/shadcn/skeleton";
import { useAdminApp } from "@/hooks/api/customApps/useCustomApps";
import { DossierBody } from "./components/AppDetail/components/Dossier";
import { DOSSIER_APP_PARAM } from "./components/AppDetail/dock";
import { useAppViewState } from "./components/AppDetail/useAppViewState";
import { DOSSIER_ROUTE_MISMATCH, dossierAppId, dossierAppMatchesRoute } from "./dossierWindowApp";
import { useAdminAppRegistry } from "./useAdminAppRegistry";

/**
 * The dossier as a standalone page, for the `window` dock mode.
 *
 * Mounted outside `AdminLayout` deliberately: this route only ever renders in a
 * popped-out window, where the admin nav chrome would be dead weight in a 560px
 * frame. The custom-app admin APIs it reads are app-admin gated server-side, so
 * an operator without standing gets empty sections rather than data.
 *
 * Its own route (rather than a React portal into `window.open`) so Radix
 * overlays inside the dossier — the rollback confirmation, copy tooltips —
 * portal into *this* window's document instead of the opener's.
 *
 * **Reads the app by id.** The opener passes `?app=<id>` (`dossierWindowPath`), and
 * this window — a separate browsing context with its own QueryClient, so none of the
 * opener's cache — reads `GET /customer-apps/{id}` for it. That is one request where
 * walking the paged registry to find a slug was one per hundred apps; it also lands
 * the detail response, which is the only one carrying `staging_url`. The walk survives
 * only as the fallback for a link without the param.
 */
export default function AppDossierWindow() {
  const { orgSlug, appSlug } = useParams<{ orgSlug: string; appSlug: string }>();
  const [params] = useSearchParams();
  // Text from the address bar, so it is a UUID or it is nothing (`dossierAppId`); a
  // malformed value falls back to the slug walk rather than reaching a request URL.
  const appId = dossierAppId(params.get(DOSSIER_APP_PARAM));

  const detail = useAdminApp(appId);
  // Disabled whenever the id is present: the walk is the fallback, not a second read.
  const registry = useAdminAppRegistry(appId ? undefined : orgSlug, appId ? undefined : appSlug);

  // The id's app must be the address's app. A mismatch is an error, not a reason to
  // show either of them.
  const mismatch =
    !!appId && !!detail.data && !dossierAppMatchesRoute(detail.data, orgSlug, appSlug);
  const selected = appId ? (mismatch ? null : (detail.data ?? null)) : registry.selected;
  const isLoading = appId ? detail.isLoading : registry.isLoading || registry.isWalking;
  const error = appId ? detail.error : registry.error;

  // This window has its own address bar and its own Back button, so it gets the
  // same URL-backed state the stage has rather than component-local `useState`.
  // The channel default is irrelevant here — the window renders no preview —
  // but sharing one binding is what stops the two surfaces disagreeing about
  // what `?section=` and `?fn=` mean.
  const { view, patch } = useAppViewState(selected?.published_at ? "published" : "draft");

  useEffect(() => {
    if (selected) document.title = `${selected.name} · Details`;
  }, [selected]);

  return (
    <div
      // Mounted outside `AdminLayout` (see above), so it is outside the console's
      // reference subtree and carries the class itself — the dossier it renders is
      // built from the admin kit, whose roles only resolve inside one.
      className={`${DESIGN_REFERENCE} flex h-screen min-h-0 flex-col bg-background`}
      data-testid='admin-app-dossier-window'
    >
      <header className='flex h-9 shrink-0 items-center gap-2 border-b px-3'>
        {selected ? (
          <>
            <span className='truncate font-medium text-xs leading-none'>{selected.name}</span>
            <span className='min-w-0 truncate font-mono text-muted-foreground/70 text-xs'>
              {selected.org_slug}/{selected.slug}
            </span>
          </>
        ) : (
          <span className='font-mono text-[11px] text-muted-foreground uppercase tracking-wider'>
            Details
          </span>
        )}
      </header>

      {selected ? (
        <DossierBody
          app={selected}
          focusSection={view.section}
          fn={view.fn}
          onFnChange={(name) => patch({ fn: name })}
        />
      ) : isLoading ? (
        <div className='space-y-3 p-4'>
          <Skeleton className='h-20 w-full' />
          <Skeleton className='h-40 w-full' />
        </div>
      ) : (
        <div
          className='flex items-center gap-2 p-4 text-destructive text-xs'
          data-testid='admin-app-dossier-window-error'
        >
          <AlertCircle className='size-4 shrink-0' />
          <span>
            {mismatch
              ? DOSSIER_ROUTE_MISMATCH
              : error
                ? appId
                  ? "Couldn't load this app."
                  : "Couldn't load the app registry."
                : `No app matches ${orgSlug}/${appSlug}. It may have been deleted.`}
          </span>
        </div>
      )}
    </div>
  );
}
