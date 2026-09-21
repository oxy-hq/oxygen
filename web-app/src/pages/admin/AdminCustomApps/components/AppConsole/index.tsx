import { ExternalLink } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { timeAgo } from "@/libs/utils/date";
import { AdminStatusPill } from "@/pages/admin/components/AdminStatusPill";
import type { CustomApp } from "@/types/apps";
import { type HealthIndex, STATUS_LABEL, statusOf } from "../../appStatus";
import { resolveBundleUrl } from "../../resolveBundleUrl";
import { Activity } from "../AppDetail/components/Activity";
import { AppAccessPane } from "../AppDetail/components/AppAccessPane";
import { AppInfo } from "../AppDetail/components/AppInfo";
import { Availability } from "../AppDetail/components/Availability";
import { BuildHistory } from "../AppDetail/components/BuildHistory";
import { Functions } from "../AppDetail/components/Functions";
import { Secrets } from "../AppDetail/components/Secrets";
import { useAppViewState } from "../AppDetail/useAppViewState";
import { AppIdentity } from "../AppIdentity";
import { statusTone } from "../AppSwitcher";
import { ConsolePanel, PanelRow } from "../ConsolePanel";
import { AppStoragePanel } from "./AppStoragePanel";
import { PreviewCard } from "./PreviewCard";
import { PublishCiPanel } from "./PublishCiPanel";

/**
 * One app, whole — the surface an operator works in, reached from the fleet at
 * `/admin/apps` or straight from the palette.
 *
 * **What it replaces.** `AppDetail` was a preview-first layout: a live iframe of the
 * customer's app owned the stage, and everything an operator actually came for —
 * status, builds, access, secrets — was a dossier of nine collapsed accordions docked
 * beside it. Opening an app showed you a website and nine closed rows.
 *
 * This inverts that. The panels **are** the page, all open, and the preview is one small
 * card with an `Open ↗`. There is no accordion component in this direction.
 *
 * **Why every panel names a question.** The console was designed against six things a
 * staff engineer arrives asking (is anything broken · what is deployed where · ship or
 * roll back · who can open this · what is it costing · let a repo publish). Each panel
 * carries the one it answers in its eyebrow. That makes the claim "one viewport answers
 * all six" checkable by reading the screen rather than by taking this comment's word for
 * it — and a panel that cannot name its question has not earned its place.
 *
 * **Why three observability panels became one.** `availability`, `logs` and `activity`
 * are all backed by the same capture pipeline. With capture off — which is the seeded
 * and, per the fleet endpoint, the current state — they were three panels each saying a
 * variant of nothing. They are one panel now, and the *reason* they are empty is stated
 * once by `FleetStrip` at the top of the surface rather than three more times here.
 */
export const AppConsole = ({
  app,
  health
}: {
  app: CustomApp;
  health: HealthIndex | undefined;
}) => {
  const status = statusOf(app, health);
  // Same URL-backed binding the dossier window and the preview stage use, so `?fn=`
  // and `?section=` name the same things across all three surfaces.
  const { view, patch } = useAppViewState(app.published_at ? "published" : "draft");
  // The app knows where it is served. Hardcoding `/customer-apps/<org>/<slug>/` here
  // re-derived one of the two forms and silently picked the wrong one for any app with
  // a subdomain — which `AppInfo`, two panels over, labels "(recommended)".
  const servedAt = app.url_subdomain ?? resolveBundleUrl(app.url);

  return (
    <div className='min-h-0 flex-1 overflow-auto' data-testid='apps-console'>
      <div className='mx-auto w-full max-w-[110rem] p-4 lg:p-6'>
        <header className='mb-4 flex flex-wrap items-start justify-between gap-3'>
          <div className='flex min-w-0 flex-col gap-1.5'>
            <AppIdentity app={app} size='lg' data-testid='apps-console-identity' />
            <div className='flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground text-xs'>
              {status ? (
                <AdminStatusPill
                  tone={statusTone(status)}
                  label={STATUS_LABEL[status]}
                  data-testid='apps-console-status'
                />
              ) : null}
              <span>
                Published{" "}
                {app.published_at ? (
                  <span title={new Date(app.published_at).toLocaleString()}>
                    {timeAgo(app.published_at)}
                  </span>
                ) : (
                  "—"
                )}
              </span>
              {/* Shown per app, unlike request counts: last-active genuinely varies
                  between these apps, so a "—" here is real data absence rather than a
                  column that can never be anything else. */}
              <span>
                Last active{" "}
                {app.last_active_at ? (
                  <span title={new Date(app.last_active_at).toLocaleString()}>
                    {timeAgo(app.last_active_at)}
                  </span>
                ) : (
                  "—"
                )}
              </span>
            </div>
          </div>
          <Button asChild variant='outline' size='sm' className='h-7 gap-1.5 text-xs'>
            <a href={servedAt} target='_blank' rel='noreferrer' data-testid='apps-console-open'>
              Open app
              <ExternalLink className='size-3' />
            </a>
          </Button>
        </header>

        {/* Two columns at width, one below. The left column is the sequence an operator
            works down — what shipped, who can reach it, what it runs — and the right is
            reference they look across at. */}
        <div className='grid items-start gap-3 xl:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)]'>
          <div className='flex min-w-0 flex-col gap-3'>
            <ConsolePanel
              id='build'
              title='Live build & history'
              question='Q2 / Q3 — what is deployed, ship or roll back'
            >
              <BuildHistory appId={app.id} />
            </ConsolePanel>

            <ConsolePanel id='access' title='Access' question='Q4 — who can open this app'>
              <AppAccessPane app={app} />
            </ConsolePanel>

            <ConsolePanel id='functions' title='Functions' question='Q1 — what it runs server-side'>
              {/* Through `useAppViewState`, not local state, so `?fn=` means the same
                  thing here as in the popped-out dossier and the preview stage — and a
                  link to one function's manifest, invocation history and Run panel is
                  shareable. This shipped as `selected={null} onSelect={() => undefined}`,
                  which rendered every row with a chevron that did nothing: exactly the
                  "looks live and isn't" defect this console calls out elsewhere. */}
              <Functions
                appId={app.id}
                selected={view.fn}
                onSelect={(name) => patch({ fn: name })}
              />
            </ConsolePanel>

            <ConsolePanel id='secrets' title='Secrets'>
              <Secrets appId={app.id} />
            </ConsolePanel>
          </div>

          <div className='flex min-w-0 flex-col gap-3'>
            <AppStoragePanel app={app} />

            <PublishCiPanel app={app} />

            <ConsolePanel id='identity' title='Identity & manifest'>
              <AppInfo app={app} />
            </ConsolePanel>

            {/* The three capture-backed views, together. Separately they were three
                panels of nothing whenever capture is off. */}
            <ConsolePanel
              id='observability'
              title='Availability & activity'
              question='Q1 — is it broken'
            >
              <div className='flex flex-col gap-3'>
                <Availability orgSlug={app.org_slug} appSlug={app.slug} />
                <Activity appId={app.id} />
              </div>
            </ConsolePanel>

            <PreviewCard app={app} servedAt={servedAt} />
          </div>
        </div>

        {/* The fleet-wide views, from inside an app. They are also linked from the
            fleet's own summary cards, which is where they are most discoverable —
            these are here so an operator deep in one app does not have to go back out
            to reach them. Links, not tabs: a tab is how this surface grew four. */}
        <p className='mt-4 flex flex-wrap gap-x-4 gap-y-1 text-muted-foreground text-xs'>
          <span>Across the fleet:</span>
          <Link
            to='/admin/apps/storage'
            className='underline underline-offset-2 hover:text-foreground'
            data-testid='apps-console-audit-link'
          >
            Storage &amp; retention
          </Link>
          <Link
            to='/admin/apps/access'
            className='underline underline-offset-2 hover:text-foreground'
            data-testid='apps-console-access-link'
          >
            Staff access &amp; lockdowns
          </Link>
        </p>
      </div>
    </div>
  );
};

export { PanelRow };
