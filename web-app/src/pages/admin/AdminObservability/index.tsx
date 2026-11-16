import { ArrowLeft, Building2, Gauge } from "lucide-react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { useObservabilityFleet } from "@/hooks/api/adminObservability/useObservabilityFleet";
import { useOrgApps } from "@/hooks/api/adminObservability/useOrgApps";
import { useOrgCost } from "@/hooks/api/adminObservability/useOrgCost";
import { useOrgResources } from "@/hooks/api/adminObservability/useOrgResources";
import ROUTES from "@/libs/utils/routes";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminPage } from "../components/AdminPage";
import { ComputePanel } from "./components/ComputePanel";
import { CostPanel } from "./components/CostPanel";
import { FleetTable } from "./components/FleetTable";
import { PerformancePanel } from "./components/PerformancePanel";
import { ResourcesPanel } from "./components/ResourcesPanel";
import { ScaleView } from "./components/ScaleView";
import { DEFAULT_WINDOW, isWindow, type WindowDays, WindowPicker } from "./components/WindowPicker";

/**
 * Admin → Observability: what an org costs us, and how its apps perform.
 *
 * **A fleet list and a per-org console on one route** — the shape `/admin/apps`
 * settled on after its own redesign, and for the stated reason: a redirect
 * answers "which one thing?" while hiding "what is out there?", and it makes
 * the URL resolve differently depending on data.
 *
 * The window lives in the URL (`?days=`) rather than in component state, so a
 * link an operator pastes into a thread shows what they were looking at.
 */
const AdminObservability = () => {
  const { orgId } = useParams<{ orgId?: string }>();
  const [params, setParams] = useSearchParams();

  const parsed = Number(params.get("days"));
  const days: WindowDays = isWindow(parsed) ? parsed : DEFAULT_WINDOW;
  const view: FleetView = params.get("view") === "scale" ? "scale" : "table";
  const setView = (next: FleetView) => {
    const update = new URLSearchParams(params);
    if (next === "table") update.delete("view");
    else update.set("view", next);
    setParams(update, { replace: true });
  };
  const setDays = (next: WindowDays) => {
    const update = new URLSearchParams(params);
    update.set("days", String(next));
    // Replace, not push: flipping between 7 and 30 is adjusting the view, not
    // navigating, and three flips should not cost three presses of Back.
    setParams(update, { replace: true });
  };

  return orgId ? (
    <OrgConsole orgId={orgId} days={days} setDays={setDays} />
  ) : (
    <FleetList days={days} setDays={setDays} view={view} setView={setView} />
  );
};

const FleetList = ({
  days,
  setDays,
  view,
  setView
}: {
  days: WindowDays;
  setDays: (d: WindowDays) => void;
  view: FleetView;
  setView: (v: FleetView) => void;
}) => {
  const fleet = useObservabilityFleet(days);
  const scale = view === "scale";

  return (
    <AdminPage
      width='wide'
      data-testid='admin-observability'
      description={
        scale
          ? "How big each organization is against the typical one, on every axis at once."
          : "What each organization costs us, and how its custom apps are performing. Pick one to see why."
      }
      actions={
        <div className='flex items-center gap-2'>
          <ViewSwitch view={view} onChange={setView} />
          <WindowPicker days={days} onChange={setDays} />
        </div>
      }
    >
      <AdminAsync
        query={fleet}
        noun='the observability fleet'
        rows={6}
        isEmpty={(d) => d.rows.length === 0}
        empty={
          <AdminEmptyState
            icon={Building2}
            title='No organizations to report on.'
            description='Orgs appear here as soon as they exist — with no spend and no traffic until they have some.'
            data-testid='admin-observability-empty'
          />
        }
      >
        {(data) => (scale ? <ScaleView fleet={data} /> : <FleetTable fleet={data} />)}
      </AdminAsync>
    </AdminPage>
  );
};

/**
 * Table or scale. A projection of one list, like the tenants directory's
 * `?type=`, so it rides a query param rather than a tab or a second route —
 * the same data answering two questions.
 */
export type FleetView = "table" | "scale";

const ViewSwitch = ({ view, onChange }: { view: FleetView; onChange: (v: FleetView) => void }) => (
  <div
    className='flex items-center gap-1'
    role='group'
    aria-label='View'
    data-testid='admin-observability-view'
  >
    {(["table", "scale"] as const).map((v) => (
      <button
        key={v}
        type='button'
        onClick={() => onChange(v)}
        aria-pressed={view === v}
        data-testid={`admin-observability-view-${v}`}
        className={
          view === v
            ? "rounded-md bg-foreground px-2 py-1 font-medium text-[11px] text-background uppercase tracking-wide"
            : "rounded-md px-2 py-1 font-medium text-[11px] text-muted-foreground uppercase tracking-wide transition-colors hover:bg-muted hover:text-foreground"
        }
      >
        {v === "table" ? "Table" : "Scale"}
      </button>
    ))}
  </div>
);

const OrgConsole = ({
  orgId,
  days,
  setDays
}: {
  orgId: string;
  days: WindowDays;
  setDays: (d: WindowDays) => void;
}) => {
  const cost = useOrgCost(orgId, days);
  const apps = useOrgApps(orgId, days);
  const resources = useOrgResources(orgId, days);
  // The org's identity rides the cost response — one indexed lookup after the
  // fence. It used to come from the fleet rollup, which joins every tenant's
  // runs over the window to answer a page about one org, and missed its cache
  // on every direct link.
  const org = cost.data?.org;

  return (
    <AdminPage
      width='wide'
      // `AdminPage` takes a title only when the page is *about* something the
      // route cannot name. An org is exactly that case.
      title={org?.name ?? "Organization"}
      data-testid='admin-observability-org'
      description={
        <span className='flex flex-wrap items-center gap-2'>
          <span>Cost to serve and custom-app performance for this organization.</span>
          <Link
            to={ROUTES.ADMIN.ORG_DETAIL(orgId)}
            className='underline underline-offset-2 hover:text-foreground'
          >
            Open the org&rsquo;s console
          </Link>
        </span>
      }
      actions={
        <div className='flex items-center gap-2'>
          <Button asChild variant='ghost' size='sm' className='h-7 gap-1.5 text-xs'>
            <Link to={`${ROUTES.ADMIN.OBSERVABILITY}?days=${days}`}>
              <ArrowLeft className='size-3.5' />
              All organizations
            </Link>
          </Button>
          <WindowPicker days={days} onChange={setDays} />
        </div>
      }
    >
      <AdminAsync query={cost} noun="this organization's spend" rows={4}>
        {(data) => <CostPanel cost={data} />}
      </AdminAsync>

      {/* Its own `AdminAsync`: the resource meters read Postgres and the
          observability store, so a failure in either must not blank the LLM
          figure above or the performance panel below. */}
      <AdminAsync query={resources} noun="this organization's resource use" rows={4}>
        {(data) => <ResourcesPanel resources={data} />}
      </AdminAsync>

      {/* Compute draws on both reads, so it renders only once both are in.
          A half-drawn composition would be a lie about the split. */}
      {apps.data?.measured ? <ComputePanel apps={apps.data} resources={resources.data} /> : null}

      {/* Two queries, two `AdminAsync`s: the cost read hits Postgres and the
          app read hits the observability store, so one being down must not
          blank the other. A single wrapper around both would hide a working
          half behind a failing one. */}
      <AdminAsync
        query={apps}
        noun="this organization's app performance"
        rows={4}
        isEmpty={(d) => d.measured && d.total.requests === 0}
        empty={
          <AdminEmptyState
            icon={Gauge}
            title='No app traffic in this window.'
            description='Measured, and nobody opened an app — which is different from not being measured at all.'
            data-testid='admin-observability-apps-empty'
          />
        }
      >
        {(data) => <PerformancePanel apps={data} orgSlug={org?.slug ?? ""} />}
      </AdminAsync>
    </AdminPage>
  );
};

export default AdminObservability;
