import { Workflow } from "lucide-react";
import { useState } from "react";
import { useDataFlow } from "@/hooks/api/customApps/useDataFlow";
import { cn } from "@/libs/shadcn/utils";
import type { DataFlow } from "@/types/dataFlow";
import { AdminAsync } from "../components/AdminAsync";
import { AdminEmptyState } from "../components/AdminEmptyState";
import { AdminKpiStrip } from "../components/AdminKpiStrip";
import { AdminPage } from "../components/AdminPage";
import { ADMIN_TONE } from "../components/adminTone";
import { DataFlowGraph } from "./components/DataFlowGraph";
import { DataFlowLegend } from "./components/DataFlowLegend";
import { DataFlowToolbar } from "./components/DataFlowToolbar";
import { NodeDetails, NodeDetailsEmpty } from "./components/NodeDetails";
import { countFlow } from "./dataFlowModel";
import { toQuery, useDataFlowParams } from "./useDataFlowParams";

const INFO = (
  <>
    Every edge is a declaration: a compiled <code>.airway.yml</code> (source → pipeline → dataset),
    and the live build of each custom app — production&rsquo;s, else staging&rsquo;s — for its
    Airhouse and OLTP schemas (<code>airhouseMigrations</code>, <code>migrations</code>), the change
    stream (<code>oltpChanges</code>), each function&rsquo;s <code>destinations</code>, watched
    tables (<code>on</code>) and pipeline uploads (<code>oxygen.sourceUploads</code>). What runs
    actually touched is not drawn yet. Functions fold into their app unless the app is focused; past
    500 nodes the graph is cut.
  </>
);

const FlowKpis = ({ flow }: { flow: DataFlow }) => {
  const counts = countFlow(flow);
  const kpi = (id: keyof typeof counts, label: string, caption?: string) => ({
    id,
    label,
    value: counts[id],
    caption,
    quiet: counts[id] === 0
  });
  return (
    <AdminKpiStrip
      data-testid='admin-data-flow-kpis'
      items={[
        kpi("sources", "Sources"),
        kpi("pipelines", "Pipelines"),
        kpi("stores", "Stores", flow.level === "table" ? "tables and schemas" : "schemas"),
        kpi("apps", "Apps"),
        kpi("edges", "Edges", "declared")
      ]}
    />
  );
};

/**
 * `/admin/data-flow` — one org's data flow, as declared: where data enters
 * (ingestion sources), what moves it (Airway pipelines, the OLTP change stream),
 * where it rests (Airhouse and OLTP schemas, named databases) and which custom
 * apps write, watch and feed it. `?focus=` keeps one node's neighbourhood, which
 * is how an app console links here.
 */
export default function AdminDataFlow() {
  const { params, update } = useDataFlowParams();
  const flow = useDataFlow(toQuery(params));
  const [selected, setSelected] = useState<string | null>(null);
  const data = flow.data;
  const selectedNode = data?.nodes.find((n) => n.id === selected) ?? null;
  const focusLabel = data?.nodes.find((n) => n.id === params.focus)?.label ?? null;

  const change = (patch: Parameters<typeof update>[0]) => {
    setSelected(null);
    update(patch);
  };

  return (
    <AdminPage
      width='full'
      description='How data moves from ingestion sources through pipelines and stores to custom apps.'
      info={INFO}
      data-testid='admin-data-flow'
    >
      <DataFlowToolbar
        params={params}
        orgName={data?.org.id === params.org ? data.org.name : null}
        workspaces={data?.workspaces ?? []}
        focusLabel={focusLabel}
        onChange={change}
      />
      {!params.org ? (
        <AdminEmptyState
          icon={Workflow}
          title='Choose an organization to see its data flow.'
          data-testid='admin-data-flow-no-org'
        />
      ) : (
        <AdminAsync
          query={flow}
          noun='the data flow'
          rows={4}
          isEmpty={(d) => d.nodes.length === 0}
          empty={
            <AdminEmptyState
              icon={Workflow}
              title='Nothing declared yet.'
              description='No compiled pipeline and no app with a live build in this organization.'
              data-testid='admin-data-flow-empty'
            />
          }
        >
          {(d) => (
            <div className='flex flex-col gap-3'>
              <FlowKpis flow={d} />
              {d.truncated && (
                <p
                  className={cn("text-xs", ADMIN_TONE.warn.text)}
                  data-testid='admin-data-flow-truncated'
                >
                  Showing the first {d.nodes.length} nodes. Focus a node to see its whole
                  neighbourhood.
                </p>
              )}
              <DataFlowLegend />
              <div
                className={cn(
                  "flex min-w-0 overflow-hidden rounded-lg border border-border/60 bg-card",
                  flow.isPlaceholderData && "opacity-60"
                )}
              >
                <DataFlowGraph
                  flow={d}
                  focus={params.focus}
                  selected={selected}
                  onSelect={setSelected}
                />
                {selectedNode ? (
                  <NodeDetails
                    flow={d}
                    node={selectedNode}
                    orgSlug={d.org.slug}
                    focused={selectedNode.id === params.focus}
                    onFocus={(id) => change({ focus: id })}
                    onSelect={setSelected}
                  />
                ) : (
                  <NodeDetailsEmpty />
                )}
              </div>
            </div>
          )}
        </AdminAsync>
      )}
    </AdminPage>
  );
}
