import { ExternalLink } from "lucide-react";
import { Link } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { useAppLifecycle } from "@/hooks/api/customApps/useAppLifecycle";
import { timeAgo } from "@/libs/utils/date";
import type { CustomApp } from "@/types/apps";
import { resolveBundleUrl } from "../../resolveBundleUrl";
import { Activity } from "../AppDetail/components/Activity";
import { AppAccessPane } from "../AppDetail/components/AppAccessPane";
import { AppInfo } from "../AppDetail/components/AppInfo";
import { Availability } from "../AppDetail/components/Availability";
import { RetainedBuilds } from "../AppDetail/components/BuildHistory/RetainedBuilds";
import { CustomDomains } from "../AppDetail/components/CustomDomains";
import { Environments } from "../AppDetail/components/Environments";
import { Functions } from "../AppDetail/components/Functions";
import { Issues, IssuesBadge } from "../AppDetail/components/Issues";
import { Publishers } from "../AppDetail/components/Publishers";
import { Releases } from "../AppDetail/components/Releases";
import { Secrets } from "../AppDetail/components/Secrets";
import { useAppViewState } from "../AppDetail/useAppViewState";
import { AppIdentity } from "../AppIdentity";
import { AppLifecycle, LifecycleVerdictPill } from "../AppLifecycle";
import { ConsolePanel } from "../ConsolePanel";
import { AppStoragePanel } from "./AppStoragePanel";
import { PreviewCard } from "./PreviewCard";
import { PublishCiPanel } from "./PublishCiPanel";
import { TestNotifications } from "./TestNotifications";

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
export const AppConsole = ({ app }: { app: CustomApp }) => {
  // The header's verdict and the lifecycle card's are one read: same hook, same cache
  // entries, so the pill beside the name can never disagree with the card below it.
  const { verdict } = useAppLifecycle(app);
  // Same URL-backed binding the dossier window and the preview stage use, so `?fn=`
  // and `?section=` name the same things across all three surfaces.
  const { view, patch } = useAppViewState(app.published_at ? "published" : "draft");
  // The app knows where it is served. Hardcoding `/customer-apps/<org>/<slug>/` here
  // re-derived one of the two forms and silently picked the wrong one for any app with
  // a subdomain — which `AppInfo`, two panels over, labels "(recommended)".
  const servedAt = app.url_subdomain ?? resolveBundleUrl(app.url);
  // An issue names a function; following it lands on that function's history —
  // its own row, not the top of a list that can run to sixty functions.
  const openFunction = (name: string) => {
    patch({ fn: name });
    requestAnimationFrame(() => {
      document
        .querySelector(`[data-testid="admin-app-function-${CSS.escape(name)}"]`)
        ?.scrollIntoView({ block: "start", behavior: "smooth" });
    });
  };

  return (
    <div className='min-h-0 flex-1 overflow-auto' data-testid='apps-console'>
      <div className='mx-auto w-full max-w-[110rem] p-4 lg:p-6'>
        <header className='mb-4 flex flex-wrap items-start justify-between gap-3'>
          <div className='flex min-w-0 flex-col gap-1.5'>
            <AppIdentity app={app} size='lg' data-testid='apps-console-identity' />
            <div className='flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground text-xs'>
              <LifecycleVerdictPill verdict={verdict} data-testid='apps-console-status' />
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

        {/* The preview leads. It is the app itself — where it is served, and the way in
            to the request log — so it is the first thing on the page and spans both
            columns rather than sitting last in the right-hand one.

            This is a deliberate revision of the original direction, which put it last on
            the argument that "a staff console is not a browser for viewing one app".
            That argument was about the old layout giving a live iframe the entire stage;
            it is not an argument for burying the app's own identity at the bottom. The
            rule it was protecting still holds and still does the work: no iframe here,
            just where it is served and two links out. */}
        <div className='mb-3'>
          <PreviewCard app={app} servedAt={servedAt} />
        </div>

        {/* Then two columns at width, one below, ordered by the six operator questions
            rather than by object type. Left is the sequence someone works down — is it
            broken, what shipped, who can reach it, what it runs — and right is the
            reference they look across at. */}
        <div className='grid items-start gap-3 xl:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)]'>
          <div className='flex min-w-0 flex-col gap-3'>
            {/* The three capture-backed views, together. Separately they were three
                panels of nothing whenever capture is off. First in the column because
                "is it broken" is the question people arrive with. */}
            <ConsolePanel
              id='observability'
              title='Availability & activity'
              question='Q1 — is it broken'
            >
              <div className='flex flex-col gap-3'>
                <Availability app={app} />
                <Activity appId={app.id} />
              </div>
            </ConsolePanel>

            {/* The failures, grouped the way the pager groups them, with each one's
                message in full. This console shipped without it — it lived only in
                the dossier this page replaced — so on 2026-10-21 the pager quoted
                fingerprints that nothing on this page could be matched against, and
                the cause sat past the truncation of each Functions row. */}
            <ConsolePanel
              id='issues'
              title='Issues'
              question='Q1 — what is failing'
              actions={<IssuesBadge appId={app.id} />}
            >
              {/* The list runs edge to edge and owns its gutter. */}
              <div className='-mx-3 -mb-3'>
                <Issues app={app} onOpenFunction={openFunction} />
              </div>
            </ConsolePanel>

            <ConsolePanel
              id='build'
              title='Live build & releases'
              question='Q2 / Q3 — what is deployed, ship or roll back'
            >
              {/* The card answers both questions and owns its verbs — promote, roll back
                  to any build, unpublish. The release timeline under it says whether the
                  last release made things worse, and rolls back a red one through the
                  same plan. The retained-build ledger is the list the card's rollback
                  chooses from, behind a disclosure. */}
              <div className='flex flex-col gap-3'>
                <AppLifecycle app={app} />
                <Releases app={app} onOpenFunction={(name) => patch({ fn: name })} />
                <RetainedBuilds appId={app.id} />
              </div>
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

            {/* Beside Functions: what its `ctx.notify` sends, tried on your own phone
                without writing or running one. */}
            <TestNotifications app={app} />
          </div>

          <div className='flex min-w-0 flex-col gap-3'>
            <AppStoragePanel app={app} />

            <PublishCiPanel app={app} />

            {/* Two backends that had no UI: the OIDC workflows allowed to publish
                (the other half of Q6, beside the CI how-to above it), and the app's
                environments — the dev sandboxes `oxyc` creates, which until now could
                be seen and torn down only from the CLI. */}
            <ConsolePanel
              id='publishers'
              title='Trusted publishers'
              question='Q6 — who may publish'
            >
              <Publishers appId={app.id} />
            </ConsolePanel>

            <ConsolePanel
              id='environments'
              title='Environments'
              question='Q2 — production, staging, sandboxes'
            >
              <Environments appId={app.id} />
            </ConsolePanel>

            <ConsolePanel id='domains' title='Custom domains'>
              <CustomDomains appId={app.id} />
            </ConsolePanel>

            <ConsolePanel id='identity' title='Identity & manifest'>
              <AppInfo app={app} />
            </ConsolePanel>
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
