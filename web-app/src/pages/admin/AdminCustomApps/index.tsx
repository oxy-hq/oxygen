import { isAxiosError } from "axios";
import { useEffect, useState } from "react";
import { Link, useLocation, useNavigate, useParams, useSearchParams } from "react-router-dom";
import { cn } from "@/libs/shadcn/utils";
import AdminPublishTokens from "@/pages/admin/AdminPublishTokens";
import { AdminAsync } from "@/pages/admin/components/AdminAsync";
import type { CustomApp } from "@/types/apps";
import { AppCockpit } from "./components/AppCockpit";
import { AppsTable } from "./components/AppsTable";
import { CreateCustomAppDialog } from "./components/CreateCustomAppDialog";
import { AccessPane } from "./components/OxyAccessPanes/AccessPane";
import StorageTab from "./components/StorageTab";
import { useAdminAppRegistry } from "./useAdminAppRegistry";

/**
 * Admin console for custom apps. Three tabs share the page shell via `?view=`:
 *   apps     (default) → custom-app registry as a full-width table;
 *                        `/admin/apps/:org/:app` opens the full-page detail.
 *   access             → per-org view ("Organizations"): each org's Oxy-access
 *                        (workspace lockdown) plus the custom apps it owns.
 *   tokens             → Publish tokens (machine-auth bearer tokens for CI).
 *
 * A selected app (the `/admin/apps/:orgSlug/:appSlug` route) always implies the
 * Apps tab, so deep links keep working regardless of `?view`. Legacy
 * `?view=orgs` / `?view=projects` links fold into the Organizations view.
 */
type View = "apps" | "access" | "tokens" | "storage";

// "Organizations" here is the per-org custom-app view — each org's Oxy-access
// (workspace lockdown) plus the apps it owns. It's scoped to the customer-apps
// surface, distinct from the cross-cutting tenant directory at /admin/tenants.
const TABS: { view: View; label: string; to: string }[] = [
  { view: "apps", label: "Apps", to: "/admin/apps" },
  { view: "access", label: "Organizations", to: "/admin/apps?view=access" },
  { view: "tokens", label: "Publish tokens", to: "/admin/apps?view=tokens" },
  { view: "storage", label: "Storage", to: "/admin/apps?view=storage" }
];

export default function AdminCustomApps() {
  const [searchParams] = useSearchParams();
  const params = useParams<{ orgSlug?: string; appSlug?: string }>();
  const view: View = params.appSlug ? "apps" : normalizeView(searchParams.get("view"));
  useLegacyHealthRedirect();

  return (
    <div data-testid='admin-customer-apps' className='flex h-[calc(100vh-3.5rem)] flex-col'>
      <AdminTabs active={view} />
      {view === "apps" ? (
        <AppsPane />
      ) : view === "access" ? (
        <AccessPane />
      ) : view === "storage" ? (
        <StorageTab />
      ) : (
        <div className='min-h-0 flex-1 overflow-auto'>
          <AdminPublishTokens embedded />
        </div>
      )}
    </div>
  );
}

/**
 * `?view=health` was a separate Health tab. Health is a column on the Apps list
 * now, so an old link lands on the list filtered to what that tab was for — the
 * apps that need someone — instead of on an unrelated default.
 */
function useLegacyHealthRedirect() {
  const [searchParams, setSearchParams] = useSearchParams();
  useEffect(() => {
    if (searchParams.get("view") !== "health") return;
    const next = new URLSearchParams(searchParams);
    next.delete("view");
    next.set("status", "attention");
    setSearchParams(next, { replace: true });
  }, [searchParams, setSearchParams]);
}

// `orgs` / `projects` are accepted for backward compatibility with bookmarked
// links from the previous three-tab layout; both resolve to the merged
// Organizations view.
const normalizeView = (v: string | null): View =>
  v === "storage"
    ? "storage"
    : v === "tokens"
      ? "tokens"
      : v === "access" || v === "orgs" || v === "projects"
        ? "access"
        : "apps";

const AdminTabs = ({ active }: { active: View }) => (
  <div className='flex items-center gap-1 border-border border-b px-2'>
    {TABS.map((t) => (
      <Link
        key={t.view}
        to={t.to}
        data-testid={`admin-apps-tab-${t.view}`}
        className={cn(
          "-mb-px border-b-2 px-2.5 py-1.5 text-xs transition-colors",
          active === t.view
            ? "border-primary font-medium text-foreground"
            : "border-transparent text-muted-foreground hover:text-foreground"
        )}
      >
        {t.label}
      </Link>
    ))}
  </div>
);

/**
 * Customer-app registry. A full-width browser (fleet strip + table/gallery) is
 * the landing; selecting an app enters the `AppCockpit` (registry rail + live
 * detail), keyed by the selected-app URL segments so deep links and the back
 * button still work.
 *
 * All pages are loaded up front so filter / sort / group operate over the
 * whole registry rather than just the first page — admin scale is dozens to
 * low hundreds, so a handful of background fetches is cheap. Revisit with
 * server-side querying only if the registry ever grows into the thousands.
 */
const AppsPane = () => {
  const navigate = useNavigate();
  const location = useLocation();
  const params = useParams<{ orgSlug?: string; appSlug?: string }>();
  const [createOpen, setCreateOpen] = useState(false);
  const { apps, selected, selectedKey, isLoading, isLoadingMore, error, refetch } =
    useAdminAppRegistry(params.orgSlug, params.appSlug);

  // If the URL referenced an app that no longer exists, drop the phantom
  // selection once loading settles. Preserve the table's query state.
  useEffect(() => {
    if (selectedKey && !isLoading && apps.length > 0 && !selected) {
      navigate({ pathname: "/admin/apps", search: location.search }, { replace: true });
    }
  }, [selectedKey, selected, isLoading, apps.length, navigate, location.search]);

  const openDetail = (app: CustomApp) =>
    navigate({
      pathname: `/admin/apps/${app.org_slug}/${app.slug}`,
      search: location.search
    });

  const closeDetail = () => navigate({ pathname: "/admin/apps", search: location.search });

  if (error && !isLoading) return <ErrorState error={error} onRetry={refetch} />;

  // A selected app enters the cockpit: a persistent registry rail beside the
  // live detail, so the operator walks the fleet without bouncing back to the
  // landing. Deep links resolve once loading settles; until then the list (with
  // its own loading state) shows.
  if (selected)
    return (
      <AppCockpit apps={apps} selected={selected} onSelect={openDetail} onBack={closeDetail} />
    );

  return (
    <div className='flex h-full min-h-0 flex-col'>
      <div className='min-h-0 flex-1'>
        <AppsTable
          apps={apps}
          isLoading={isLoading}
          isLoadingMore={isLoadingMore}
          onSelect={openDetail}
          onCreate={() => setCreateOpen(true)}
        />
      </div>
      <CreateCustomAppDialog open={createOpen} onOpenChange={setCreateOpen} />
    </div>
  );
};

/**
 * A 403 keeps its own copy: it names the one thing the operator can fix without
 * paging anyone, which the generic failure cannot. Everything else goes through
 * `AdminAsync`, so it prints the server's own message and offers the Retry this
 * block never had. Only the failure is routed through the kit — loading stays
 * with `AppsTable`, which renders it inside the full-viewport browser frame.
 */
const ErrorState = ({ error, onRetry }: { error: unknown; onRetry: () => void }) =>
  isAxiosError(error) && error.response?.status === 403 ? (
    <div className='mx-auto max-w-2xl p-6'>
      <div className='rounded-lg border border-destructive/30 bg-destructive/5 p-6 text-center'>
        <p className='font-medium text-destructive text-xs'>
          Your account isn't on the custom-apps allow list.
        </p>
        <p className='mt-2 text-muted-foreground text-xs'>
          Add your email to the oxy backend's{" "}
          <code className='rounded bg-muted px-1 py-0.5 font-mono'>OXY_GLOBAL_ADMINS</code> env var
          (comma-separated) and restart the server, then refresh.
        </p>
      </div>
    </div>
  ) : (
    <AdminAsync
      className='mx-auto max-w-2xl p-6'
      query={{ isError: true, data: undefined, error, refetch: onRetry }}
      noun='apps'
    >
      {() => null}
    </AdminAsync>
  );
