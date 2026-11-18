import { CircleSlash, ExternalLink } from "lucide-react";
import { useEffect } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { useAppLifecycle } from "@/hooks/api/customApps/useAppLifecycle";
import { timeAgo } from "@/libs/utils/date";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { PRODUCTION } from "@/types/appEnvironments";
import type { CustomApp } from "@/types/apps";
import { resolveBundleUrl } from "../../resolveBundleUrl";
import { useAppViewState } from "../AppDetail/useAppViewState";
import { AppIdentity } from "../AppIdentity";
import { LifecycleVerdictPill } from "../AppLifecycle";
import { ConsoleNav } from "./ConsoleNav";
import { type ConsolePageId, consolePageFrom, consolePagePath } from "./consolePages";
import { EnvironmentStrip } from "./EnvironmentStrip";
import { EventsPage } from "./Events";
import { environmentScope } from "./environmentScope";
import { ActivityPage } from "./pages/ActivityPage";
import { DataPage } from "./pages/DataPage";
import { FunctionsPage } from "./pages/FunctionsPage";
import { OverviewPage } from "./pages/OverviewPage";
import { ReleasesPage } from "./pages/ReleasesPage";
import { SecretsPage } from "./pages/SecretsPage";
import { SettingsPage } from "./pages/SettingsPage";

/**
 * One app, whole: the surface an operator works in, reached from the fleet at
 * `/admin/apps` or straight from the palette.
 *
 * **Pages, not one scroll.** This console was one page of twenty open panels, each
 * labelled with the operator question it answered ("Q1 — is it broken") and most with a
 * paragraph under the title. Everything was on screen, so everything had to be read
 * past. It is now Encore's shape: an app sidebar, an overview that leads with five
 * numbers and what is broken, and one short page per job (`consolePages.ts`). Each page
 * has its own URL, so "the dead letters on store-ops" is a link.
 *
 * **What stayed.** The environment strip rules every page that is one environment's
 * (`?env=`); panels still say which environment they show; and explanations still
 * exist, behind an ⓘ on the title they explain rather than under it.
 */
export const AppConsole = ({ app }: { app: CustomApp }) => {
  const { consolePage } = useParams<{ consolePage?: string }>();
  const page = consolePageFrom(consolePage);
  const navigate = useNavigate();
  const { search } = useLocation();
  // The header's verdict and the lifecycle card's are one read: same hook, same cache
  // entries, so the pill beside the name can never disagree with the card below it.
  const { verdict } = useAppLifecycle(app);
  // Same URL-backed binding the dossier window and the preview stage use, so `?fn=`
  // and `?env=` name the same things across all three surfaces.
  const { view, patch } = useAppViewState(app.published_at ? "published" : "draft");
  // The app knows where it is served; its subdomain, when it has one, is the form
  // `AppInfo` labels "(recommended)".
  const servedAt = app.url_subdomain ?? resolveBundleUrl(app.url);
  const environment = view.env ?? PRODUCTION;
  const here = environmentScope(environment);
  const setEnvironment = (next: string) => patch({ env: next === PRODUCTION ? null : next });
  const scope = { label: here.label, warn: here.tone === "warn" };

  // An issue or a node on the app map names a function; following it opens Functions
  // on that function's own row, not the top of a list that can run to sixty.
  const openFunction = (name: string) => {
    navigate(consolePagePath(app, "functions", search, { fn: name }));
  };
  useEffect(() => {
    if (page !== "functions" || !view.fn) return;
    const frame = requestAnimationFrame(() => {
      document
        .querySelector(`[data-testid="admin-app-function-${CSS.escape(view.fn as string)}"]`)
        ?.scrollIntoView({ block: "start", behavior: "smooth" });
    });
    return () => cancelAnimationFrame(frame);
  }, [page, view.fn]);

  return (
    <div className='min-h-0 flex-1 overflow-auto' data-testid='apps-console'>
      <div className='mx-auto w-full max-w-[110rem] p-4 lg:p-6'>
        <EnvironmentStrip appId={app.id} value={environment} onChange={setEnvironment} />
        <header className='mb-4 flex flex-wrap items-start justify-between gap-3'>
          <div className='flex min-w-0 flex-col gap-1.5'>
            <AppIdentity app={app} size='lg' data-testid='apps-console-identity' />
            <div className='flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground text-xs'>
              <LifecycleVerdictPill verdict={verdict} data-testid='apps-console-status' />
              <Since label='Published' at={app.published_at} />
              <Since label='Last active' at={app.last_active_at} />
            </div>
          </div>
          <Button asChild variant='outline' size='sm' className='h-7 gap-1.5 text-xs'>
            <a href={servedAt} target='_blank' rel='noreferrer' data-testid='apps-console-open'>
              Open app
              <ExternalLink className='size-3' />
            </a>
          </Button>
        </header>

        <div className='flex gap-6 max-lg:flex-col max-lg:gap-3'>
          <ConsoleNav app={app} page={page ?? "overview"} environment={environment} />
          <main
            className='min-w-0 flex-1'
            data-testid='apps-console-main'
            data-page={page ?? "missing"}
          >
            {page === null ? (
              <AdminEmptyState
                icon={CircleSlash}
                title={`This app's console has no "${consolePage}" page.`}
                action={
                  <Button asChild variant='outline' size='sm' className='h-7 text-xs'>
                    <Link to={consolePagePath(app, "overview", search)}>Overview</Link>
                  </Button>
                }
                data-testid='apps-console-page-missing'
              />
            ) : (
              <ConsolePageBody
                page={page}
                app={app}
                servedAt={servedAt}
                environment={environment}
                scope={scope}
                selectedFunction={view.fn ?? null}
                onSelectFunction={(name) => patch({ fn: name })}
                onOpenFunction={openFunction}
                onEnvironmentChange={setEnvironment}
              />
            )}
          </main>
        </div>
      </div>
    </div>
  );
};

const Since = ({ label, at }: { label: string; at: string | null | undefined }) => (
  <span>
    {label} {at ? <span title={new Date(at).toLocaleString()}>{timeAgo(at)}</span> : "—"}
  </span>
);

const ConsolePageBody = ({
  page,
  app,
  servedAt,
  environment,
  scope,
  selectedFunction,
  onSelectFunction,
  onOpenFunction,
  onEnvironmentChange
}: {
  page: ConsolePageId;
  app: CustomApp;
  servedAt: string;
  environment: string;
  scope: { label: string; warn?: boolean };
  selectedFunction: string | null;
  onSelectFunction: (name: string | null) => void;
  onOpenFunction: (name: string) => void;
  onEnvironmentChange: (next: string) => void;
}) => {
  switch (page) {
    case "overview":
      return <OverviewPage app={app} servedAt={servedAt} onOpenFunction={onOpenFunction} />;
    case "activity":
      return (
        <ActivityPage
          app={app}
          environment={environment}
          scope={scope}
          onOpenFunction={onOpenFunction}
        />
      );
    case "functions":
      return (
        <FunctionsPage
          app={app}
          environment={environment}
          scope={scope}
          selected={selectedFunction}
          onSelect={onSelectFunction}
          onOpenFunction={onOpenFunction}
        />
      );
    case "events":
      return <EventsPage app={app} environment={environment} scope={scope} />;
    case "releases":
      return <ReleasesPage app={app} onOpenFunction={onOpenFunction} />;
    case "data":
      return <DataPage app={app} environment={environment} scope={scope} />;
    case "secrets":
      return (
        <SecretsPage
          app={app}
          environment={environment}
          scope={scope}
          onEnvironmentChange={onEnvironmentChange}
        />
      );
    case "settings":
      return <SettingsPage app={app} />;
  }
};
