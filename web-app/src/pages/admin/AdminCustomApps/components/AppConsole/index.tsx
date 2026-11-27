import { CircleSlash } from "lucide-react";
import { useEffect } from "react";
import { Link, useLocation, useNavigate, useParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { useAppBuilds } from "@/hooks/api/customApps/useAppBuilds";
import { useAppLifecycle } from "@/hooks/api/customApps/useAppLifecycle";
import { AdminEmptyState } from "@/pages/admin/components/AdminEmptyState";
import { PRODUCTION } from "@/types/appEnvironments";
import type { CustomApp } from "@/types/apps";
import { resolveBundleUrl } from "../../resolveBundleUrl";
import { useAppViewState } from "../AppDetail/useAppViewState";
import { ConsoleHeader } from "./ConsoleHeader";
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
 *
 * **`railed` means "the column browser is around me".** It suppresses this console's own
 * sidebar at `lg` and up, because there the browser's Pages tier is that sidebar — same
 * list, same icons, same counts, drawn once. Everything else is unchanged, including the
 * header: the pane is where an operator reads the app's name and verdict, and the
 * browser's path bar names it in the file-browser's own idiom rather than repeating it.
 */
export const AppConsole = ({ app, railed = false }: { app: CustomApp; railed?: boolean }) => {
  const { consolePage } = useParams<{ consolePage?: string }>();
  const page = consolePageFrom(consolePage);
  const navigate = useNavigate();
  const { search } = useLocation();
  // The header's verdict and the lifecycle card's are one read: same hook, same cache
  // entries, so the pill beside the name can never disagree with the card below it.
  const { verdict } = useAppLifecycle(app);
  // A publish from CI or a terminal moves staging with nothing on this page to say
  // so: keep the builds every panel reads fresh while the console is open.
  useAppBuilds(app.id, { watch: true });
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
        <ConsoleHeader app={app} verdict={verdict} servedAt={servedAt} />

        <div className='flex gap-6 max-lg:flex-col max-lg:gap-3'>
          {/* Inside the column browser the Pages tier *is* this nav at `lg` and up, from
              the same `CONSOLE_PAGES` and the same `useConsoleBadges`. It stays below
              `lg`, where the browser does not render at all. */}
          <ConsoleNav
            app={app}
            page={page ?? "overview"}
            environment={environment}
            className={railed ? "lg:hidden" : undefined}
          />
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
