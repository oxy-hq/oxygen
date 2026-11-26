import { ArrowLeft, ArrowUpCircle, ServerCog } from "lucide-react";
import type React from "react";
import { useMemo } from "react";
import { Link, useParams, useSearchParams } from "react-router-dom";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import useFleetMembers from "@/hooks/api/cameras/useFleetMembers";
import ROUTES from "@/libs/utils/routes";
import type { EdgeBoxSummary } from "@/services/api/cameras";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import BudgetPanel from "./components/BudgetPanel";
import LogsPanel from "./components/LogsPanel";
import UtilizationPanel from "./components/UtilizationPanel";

/**
 * Unified troubleshooting surface for one edge box. Replaces the
 * popup `LogViewerDialog` for the day-to-day "is this box healthy"
 * use case and gives us a single place to bolt utilization, budget,
 * and recovery affordances on top in subsequent steps.
 *
 * Layout: a persistent header strip + a tabbed body. Header stays
 * pinned across tab switches so the operator never loses the
 * status/auth-mode/last-seen context while drilling into logs or
 * utilization. The active tab is driven by `?tab=` so a back-button
 * round-trip preserves the operator's place, and the
 * `EdgeBoxesTable` "view logs" affordance deep-links into the right
 * tab directly. A legacy `#logs` hash falls through to the same
 * default in case any bookmarks survive from the pre-tabs layout.
 *
 * Data is sourced from `useFleetMembers` so deep-links from the
 * boxes table hit the existing cache — the operator almost always
 * arrives via the table row.
 */
const TABS = ["logs", "utilization", "budget", "troubleshooting"] as const;
type TabId = (typeof TABS)[number];
const DEFAULT_TAB: TabId = "logs";

const statusVariant = (status: string): "default" | "secondary" | "destructive" | "outline" => {
  switch (status) {
    case "active":
      return "default";
    case "offline":
      return "destructive";
    case "retired":
      return "outline";
    default:
      return "secondary";
  }
};

const isTabId = (s: string | null): s is TabId =>
  s !== null && (TABS as readonly string[]).includes(s);

const EdgeBoxDetailPage: React.FC = () => {
  const { boxId } = useParams<{ boxId: string }>();
  const { workspace } = useCurrentWorkspace();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const back = ROUTES.ORG(orgSlug).WORKSPACE().IDE.EDGE.DEVICES;

  const [searchParams, setSearchParams] = useSearchParams();
  const tabParam = searchParams.get("tab");
  const activeTab: TabId = isTabId(tabParam) ? tabParam : DEFAULT_TAB;
  const onTabChange = (next: string) => {
    if (!isTabId(next)) return;
    // `replace` so tab switches don't pile up in browser history —
    // the operator's Back button should leave the detail page, not
    // walk through every tab they previewed.
    setSearchParams(
      (prev) => {
        const params = new URLSearchParams(prev);
        if (next === DEFAULT_TAB) params.delete("tab");
        else params.set("tab", next);
        return params;
      },
      { replace: true }
    );
  };

  const { data: fleetMembers = [], isLoading, error } = useFleetMembers(workspace?.id);

  const box: EdgeBoxSummary | null = useMemo(() => {
    if (!boxId) return null;
    for (const m of fleetMembers) {
      if (m.edge_box?.id === boxId) {
        return { ...m.edge_box, site_name: m.site_name };
      }
    }
    return null;
  }, [boxId, fleetMembers]);

  return (
    <div className='flex flex-col gap-4 p-4'>
      <div className='flex items-center gap-2'>
        <Button asChild variant='ghost' size='sm'>
          <Link to={back}>
            <ArrowLeft className='mr-1.5 h-3.5 w-3.5' />
            Boxes
          </Link>
        </Button>
      </div>

      {isLoading && !box ? (
        <HeaderSkeleton />
      ) : error ? (
        <ErrorState message={error.message} />
      ) : !box ? (
        <NotFoundState boxId={boxId ?? ""} />
      ) : (
        <Header box={box} />
      )}

      <Tabs value={activeTab} onValueChange={onTabChange} className='flex flex-col gap-3'>
        <TabsList className='self-start'>
          <TabsTrigger value='logs'>Logs</TabsTrigger>
          <TabsTrigger value='utilization'>Utilization</TabsTrigger>
          <TabsTrigger value='budget'>Budget</TabsTrigger>
          <TabsTrigger value='troubleshooting'>Troubleshooting</TabsTrigger>
        </TabsList>

        <TabsContent value='logs' className='m-0'>
          <Panel title='Logs' subtitle='Streaming from the edge worker · server-side filters'>
            <LogsPanel workspaceId={workspace?.id} boxId={boxId} />
          </Panel>
        </TabsContent>

        <TabsContent value='utilization' className='m-0'>
          {box ? (
            <Panel
              title='Utilization'
              subtitle='Postgres-side fields the worker stamps on heartbeat'
            >
              <UtilizationPanel box={box} />
            </Panel>
          ) : (
            <Placeholder
              title='Utilization'
              description='Box not loaded — utilization requires the box record.'
            />
          )}
        </TabsContent>

        <TabsContent value='budget' className='m-0'>
          <Panel title='Budget' subtitle='Per-box VLM spend + workspace context'>
            <BudgetPanel workspaceId={workspace?.id} boxId={boxId} />
          </Panel>
        </TabsContent>

        <TabsContent value='troubleshooting' className='m-0'>
          <Placeholder
            title='Troubleshooting'
            description='Recent restarts, update history, claim status, RTSP probe, install-log link.'
          />
        </TabsContent>
      </Tabs>
    </div>
  );
};

const Header: React.FC<{ box: EdgeBoxSummary }> = ({ box }) => {
  const updatePending =
    box.target_image_tag != null && box.target_image_tag !== box.current_image_tag;
  return (
    <div className='flex flex-col gap-3 border border-border bg-card p-4'>
      <div className='flex items-start gap-3'>
        <div className='bg-muted p-2'>
          <ServerCog className='h-5 w-5 text-muted-foreground' />
        </div>
        <div className='flex min-w-0 flex-1 flex-col gap-1'>
          <div className='flex flex-wrap items-baseline gap-x-2 gap-y-0.5'>
            <h1 className='truncate font-semibold text-lg'>{box.hardware_model}</h1>
            <span className='text-muted-foreground text-sm'>· {box.site_name}</span>
          </div>
          <div className='flex flex-wrap items-center gap-1.5'>
            <Badge variant={statusVariant(box.status)}>{box.status}</Badge>
            {box.incompatible_reason && (
              <Badge
                variant='outline'
                className='gap-1 border-dot-warning/40 bg-dot-warning/10 text-[10px] text-status-warning'
                title={box.incompatible_reason}
              >
                needs upgrade
              </Badge>
            )}
          </div>
        </div>
      </div>

      <dl className='grid grid-cols-2 gap-x-4 gap-y-2 text-xs md:grid-cols-4'>
        <Field label='Last seen'>
          {box.last_seen_at ? new Date(box.last_seen_at).toLocaleString() : "Never"}
        </Field>
        <Field label='Image'>
          <code className='text-xs'>{box.current_image_tag ?? "(not reported)"}</code>
          {updatePending && (
            <Badge variant='outline' className='mt-1 gap-1 self-start'>
              <ArrowUpCircle className='h-3 w-3' />→ {box.target_image_tag}
            </Badge>
          )}
        </Field>
        <Field label='Cohort'>{box.cohort || "—"}</Field>
        <Field label='Last update'>
          {box.last_update_result ? (
            <>
              <span>{box.last_update_result}</span>
              {box.last_update_at && (
                <span className='block text-[11px] text-muted-foreground'>
                  {new Date(box.last_update_at).toLocaleString()}
                </span>
              )}
            </>
          ) : (
            "—"
          )}
        </Field>
      </dl>
    </div>
  );
};

const Field: React.FC<{ label: string; children: React.ReactNode }> = ({ label, children }) => (
  <div className='flex min-w-0 flex-col gap-0.5'>
    <dt className='text-[10px] text-muted-foreground uppercase tracking-wide'>{label}</dt>
    <dd className='break-all text-foreground'>{children}</dd>
  </div>
);

const Panel: React.FC<{ title: string; subtitle?: string; children: React.ReactNode }> = ({
  title,
  subtitle,
  children
}) => (
  <div className='flex flex-col gap-3 border border-border bg-card p-4'>
    <div className='flex items-center justify-between'>
      <h2 className='font-semibold text-base'>{title}</h2>
      {subtitle && <span className='text-muted-foreground text-xs'>{subtitle}</span>}
    </div>
    {children}
  </div>
);

const Placeholder: React.FC<{ title: string; description: string }> = ({ title, description }) => (
  <div className='flex flex-col gap-1 border border-border border-dashed bg-card p-4'>
    <h2 className='font-semibold text-base'>{title}</h2>
    <p className='text-muted-foreground text-sm'>{description}</p>
  </div>
);

const HeaderSkeleton: React.FC = () => (
  <div className='flex animate-pulse flex-col gap-3 border border-border bg-card p-4'>
    <div className='h-5 w-48 bg-muted' />
    <div className='h-3 w-32 bg-muted' />
  </div>
);

const ErrorState: React.FC<{ message: string }> = ({ message }) => (
  <div className='border border-destructive bg-destructive/10 p-3 text-destructive text-sm'>
    Failed to load fleet: {message}
  </div>
);

const NotFoundState: React.FC<{ boxId: string }> = ({ boxId }) => (
  <div className='border border-border bg-card p-4 text-muted-foreground text-sm'>
    No edge box with id <code className='font-mono'>{boxId}</code> in this workspace. It may have
    been retired or you may have switched workspaces.
  </div>
);

export default EdgeBoxDetailPage;
