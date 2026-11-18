import { ArrowLeft, Building2, CalendarClock, Gauge, Search } from "lucide-react";
import { useState } from "react";
import { Link, useMatch, useParams, useSearchParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
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
import { FleetOverview } from "./components/FleetOverview";
import { OrgStanding } from "./components/OrgStanding";
import { PerformancePanel } from "./components/PerformancePanel";
import { ResourcesPanel } from "./components/ResourcesPanel";
import { DEFAULT_WINDOW, isWindow, type WindowDays, WindowPicker } from "./components/WindowPicker";
import { WeeklyUsage } from "./weekly/WeeklyUsage";

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
  // The search lives in the URL too, so a pasted link shows the same orgs.
  // A retired `?view=scale` from the old toggle is ignored, not an error.
  const query = params.get("q") ?? "";
  const setQuery = (next: string) => {
    const update = new URLSearchParams(params);
    if (next) update.set("q", next);
    else update.delete("q");
    setParams(update, { replace: true });
  };
  const setDays = (next: WindowDays) => {
    const update = new URLSearchParams(params);
    update.set("days", String(next));
    // Replace, not push: flipping between 7 and 30 is adjusting the view, not
    // navigating, and three flips should not cost three presses of Back.
    setParams(update, { replace: true });
  };

  // Last week's usage report: a view of this surface, routed (`/weekly`) rather than a
  // tab, like `/admin/apps/storage`. It has its own fixed period — the report's week — so
  // the window picker does not apply to it.
  const weekly = useMatch(ROUTES.ADMIN.OBSERVABILITY_WEEKLY);
  if (weekly) return <WeeklyUsage />;

  return orgId ? (
    <OrgConsole orgId={orgId} days={days} setDays={setDays} />
  ) : (
    <FleetList days={days} setDays={setDays} query={query} setQuery={setQuery} />
  );
};

const FleetList = ({
  days,
  setDays,
  query,
  setQuery
}: {
  days: WindowDays;
  setDays: (d: WindowDays) => void;
  query: string;
  setQuery: (q: string) => void;
}) => {
  const fleet = useObservabilityFleet(days);

  return (
    <AdminPage
      width='wide'
      data-testid='admin-observability'
      description='What each organization costs us to serve, and what it did for it.'
      actions={
        <div className='flex items-center gap-2'>
          <Button asChild variant='ghost' size='sm' className='h-7 gap-1.5 text-xs'>
            <Link to={ROUTES.ADMIN.OBSERVABILITY_WEEKLY} data-testid='admin-observability-weekly'>
              <CalendarClock className='size-3.5' />
              Weekly app usage
            </Link>
          </Button>
          <div className='relative w-56'>
            <Search className='pointer-events-none absolute top-1/2 left-2 size-3.5 -translate-y-1/2 text-muted-foreground' />
            <Input
              type='search'
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder='Find an organization'
              aria-label='Find an organization'
              className='h-7 pl-7 text-xs'
              data-testid='admin-observability-search'
            />
          </div>
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
        {(data) => <FleetOverview fleet={data} query={query} />}
      </AdminAsync>
    </AdminPage>
  );
};

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
  // The fleet read places this org among the rest. Ranking needs every org's
  // cost to serve, so it is a fleet-wide read: used when the operator came
  // from the list and it is cached, and otherwise run only when asked — a
  // direct link to one org does not pay for every tenant's.
  const [placeAmongFleet, setPlaceAmongFleet] = useState(false);
  const fleet = useObservabilityFleet(days, { enabled: placeAmongFleet });
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
      {fleet.data || placeAmongFleet ? (
        <AdminAsync query={fleet} noun="this organization's cost to serve" rows={2}>
          {(data) => <OrgStanding orgId={orgId} fleet={data} />}
        </AdminAsync>
      ) : (
        <div className='flex items-center gap-2 text-muted-foreground text-xs'>
          <span>Its rank by cost to serve needs a read of every organization.</span>
          <Button
            variant='outline'
            size='sm'
            className='h-7 text-xs'
            onClick={() => setPlaceAmongFleet(true)}
            data-testid='admin-observability-place-among-fleet'
          >
            Show its place in the fleet
          </Button>
        </div>
      )}

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
