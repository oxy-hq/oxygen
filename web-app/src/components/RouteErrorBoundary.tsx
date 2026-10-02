import * as Sentry from "@sentry/react";
import { AlertTriangle, RefreshCw } from "lucide-react";
import { useEffect, useState } from "react";
import { isRouteErrorResponse, useRouteError } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { APP_VERSION, fetchDeployedVersion } from "@/libs/utils/appVersion";

const AUTO_RELOAD_AT_KEY = "route-error-auto-reload-at";
const AUTO_RELOAD_COOLDOWN_MS = 60_000;
const AUTO_RELOAD_COUNTDOWN_S = 10;

const reloadPage = () => {
  sessionStorage.setItem(AUTO_RELOAD_AT_KEY, String(Date.now()));
  window.location.reload();
};

// Result of confirming the error against the deployed version:
// - "checking": the /version.json fetch is still in flight.
// - "outdated": a different build is live — a new version shipped since this tab
//   loaded, so the error is a stale-build artifact and reloading fixes it.
// - "current": the same build is deployed (or the version couldn't be read) —
//   the error is genuine and belongs to the latest code.
type VersionVerdict = "checking" | "outdated" | "current";

/**
 * Router-level `errorElement`.
 *
 * React Router catches any render-phase throw within a route subtree and renders
 * the nearest `errorElement`. With none defined it falls back to its bare-bones
 * default screen ("Unexpected Application Error!… provide your own ErrorBoundary
 * or errorElement"). Crucially, the app-level Sentry `<ErrorBoundary>` wrapping
 * `<RouterProvider>` never sees these — React Router intercepts route render
 * errors first — so a single unguarded throw (e.g. a bad timestamp reaching
 * date-fns) took the whole page down to that raw screen.
 *
 * Before blaming the code, this always checks the deployed version: a route
 * error in a tab that predates the latest deploy (a rotated chunk, an API
 * contract change, …) is a stale-build symptom, not a bug. Only once the running
 * build is confirmed current do we present a crash and report it to Sentry.
 */
export function RouteErrorBoundary() {
  const error = useRouteError();
  const [verdict, setVerdict] = useState<VersionVerdict>("checking");

  // Always confirm against the deployed version first. Only claim a new version
  // when we can positively read a different one; an unreadable version.json
  // (null) is treated as the current build so a genuine crash isn't mislabelled
  // as an update.
  useEffect(() => {
    let cancelled = false;
    // fetchDeployedVersion never rejects: any failure resolves to null.
    void fetchDeployedVersion().then((deployed) => {
      if (!cancelled) {
        setVerdict(deployed !== null && deployed !== APP_VERSION ? "outdated" : "current");
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // Report to Sentry only once the error is confirmed to belong to the current
  // build — never expected navigation (a thrown Response), and never a
  // stale-deploy error (which would otherwise spam Sentry after every release).
  useEffect(() => {
    if (verdict === "current" && !isRouteErrorResponse(error)) {
      Sentry.captureException(error);
    }
  }, [verdict, error]);

  // Arm the auto-reload only once a new version is confirmed live, and at most
  // once per cooldown so a persistently-broken load can't reload-loop.
  const autoReload =
    verdict === "outdated" &&
    Date.now() - Number(sessionStorage.getItem(AUTO_RELOAD_AT_KEY) ?? "0") >
      AUTO_RELOAD_COOLDOWN_MS;
  const [secondsLeft, setSecondsLeft] = useState(AUTO_RELOAD_COUNTDOWN_S);

  useEffect(() => {
    if (!autoReload) return;
    if (secondsLeft <= 0) {
      reloadPage();
      return;
    }
    const timer = window.setTimeout(() => setSecondsLeft((s) => s - 1), 1000);
    return () => window.clearTimeout(timer);
  }, [autoReload, secondsLeft]);

  if (verdict === "checking") {
    return (
      <div className='flex min-h-screen w-full flex-col items-center justify-center gap-3 bg-background p-6 text-center'>
        <Spinner className='size-6' />
        <p className='text-muted-foreground text-sm'>Checking for updates…</p>
      </div>
    );
  }

  if (verdict === "outdated") {
    return (
      <div className='flex min-h-screen w-full flex-col items-center justify-center gap-4 bg-background p-6 text-center'>
        <RefreshCw className='h-10 w-10 text-primary' />
        <div className='space-y-1'>
          <h1 className='font-semibold text-lg'>A new version is available</h1>
          <p className='max-w-md text-muted-foreground text-sm'>
            Oxygen has been updated since this page was loaded. Reload to get the latest version.
          </p>
        </div>
        <Button onClick={reloadPage}>Reload now</Button>
        {autoReload && (
          <p className='text-muted-foreground text-xs'>
            Reloading automatically in {secondsLeft}s…
          </p>
        )}
      </div>
    );
  }

  // verdict === "current": a genuine error on the latest build.
  const detail = isRouteErrorResponse(error)
    ? `${error.status} ${error.statusText}`
    : error instanceof Error
      ? error.message
      : undefined;

  return (
    <div className='flex min-h-screen w-full flex-col items-center justify-center gap-4 bg-background p-6 text-center'>
      <AlertTriangle className='h-10 w-10 text-destructive' />
      <div className='space-y-1'>
        <h1 className='font-semibold text-lg'>Something went wrong</h1>
        <p className='max-w-md text-muted-foreground text-sm'>
          This page hit an unexpected error. Your work is safe — reloading usually fixes it.
        </p>
      </div>
      {detail && (
        <pre className='max-w-md overflow-auto rounded-md bg-muted px-3 py-2 text-left text-muted-foreground text-xs'>
          {detail}
        </pre>
      )}
      <div className='flex items-center gap-2'>
        <Button variant='outline' onClick={() => window.location.reload()}>
          Reload page
        </Button>
        <Button
          onClick={() => {
            window.location.href = "/";
          }}
        >
          Go home
        </Button>
      </div>
    </div>
  );
}
