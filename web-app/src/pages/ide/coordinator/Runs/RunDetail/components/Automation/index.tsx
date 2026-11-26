import { Flame } from "lucide-react";
import type React from "react";
import { useEffect, useMemo, useState } from "react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import type { DagStepSummary, RunEventEntry } from "@/services/api/coordinator";
import { formatDurationMs } from "../../../../components/utils";
import { AgentEventLog } from "../AgentEventLog";
import { AutomationGraph } from "./AutomationGraph";
import { type AutomationNode, buildAutomationModel } from "./model";
import { StepInspector } from "./StepInspector";

/**
 * Automation (DAG) run detail body — replaces the generic TaskTree for
 * `source_type = "automation"` runs. Three tabs:
 *   - **Graph** (default): top-down step chain with status colors and
 *     a side inspector for the selected node.
 *   - **Gantt**: the existing agent waterfall renderer pointed at the
 *     automation's event log; useful for spotting parallelism (when we
 *     grow it) and per-event timings.
 *   - **Events**: raw filtered event log as a fallback.
 */
export const AutomationBody: React.FC<{
  steps: DagStepSummary[];
  events: RunEventEntry[];
}> = ({ steps, events }) => {
  const model = useMemo(() => buildAutomationModel(steps, events), [steps, events]);
  const [selectedName, setSelectedName] = useState<string | null>(null);

  // Auto-select the first leaf on first render so the inspector
  // immediately shows useful content (a top-level container would
  // render the "click a child" prompt — slightly worse first read).
  useEffect(() => {
    if (selectedName) return;
    const firstLeaf = (ns: AutomationNode[]): AutomationNode | null => {
      for (const n of ns) {
        if (n.children.length === 0) return n;
        const inner = firstLeaf(n.children);
        if (inner) return inner;
      }
      return null;
    };
    const pick = firstLeaf(model.nodes) ?? model.nodes[0];
    if (pick) setSelectedName(pick.name);
  }, [model.nodes, selectedName]);

  // Recursive lookup by name — handles container children too. Inline
  // inside useMemo so the closure captures fresh nodes without needing
  // a separate useCallback wrapper.
  const selectedNode = useMemo<AutomationNode | null>(() => {
    if (!selectedName) return null;
    const find = (ns: AutomationNode[]): AutomationNode | null => {
      for (const n of ns) {
        if (n.name === selectedName) return n;
        if (n.children.length > 0) {
          const hit = find(n.children);
          if (hit) return hit;
        }
      }
      return null;
    };
    return find(model.nodes);
  }, [model.nodes, selectedName]);

  // Roll status counts across the *entire* tree so containers and
  // pending leaves are reflected in the header chips, not just the
  // top-level row.
  const allNodes: typeof model.nodes = [];
  const walk = (ns: typeof model.nodes): void => {
    for (const n of ns) {
      allNodes.push(n);
      if (n.children.length > 0) walk(n.children);
    }
  };
  walk(model.nodes);

  const critical = model.criticalNode;
  const succeeded = allNodes.filter((n) => n.status === "succeeded").length;
  const failed = allNodes.filter((n) => n.status === "failed").length;
  const cached = allNodes.filter((n) => n.cached).length;
  const pending = allNodes.filter((n) => n.status === "pending").length;

  return (
    <Tabs defaultValue='graph' className='gap-0'>
      <div className='flex items-center justify-between border-border border-b px-4 py-2'>
        <TabsList>
          <TabsTrigger value='graph'>Graph</TabsTrigger>
          <TabsTrigger value='events'>Events</TabsTrigger>
        </TabsList>
        <div className='flex items-center gap-3 text-muted-foreground text-xs'>
          <span className='tabular-nums'>{allNodes.length} steps</span>
          {succeeded > 0 && <span className='text-status-success'>✓ {succeeded}</span>}
          {cached > 0 && <span className='text-status-info'>cached {cached}</span>}
          {failed > 0 && <span className='text-destructive'>✗ {failed}</span>}
          {pending > 0 && <span>pending {pending}</span>}
          {critical && (
            <span className='flex items-center gap-1 text-status-warning'>
              <Flame className='h-3 w-3' />
              critical {critical.name} · {formatDurationMs(critical.durationMs ?? 0)}
            </span>
          )}
        </div>
      </div>

      <TabsContent value='graph' className='mt-0'>
        <div className='flex flex-col gap-4 p-3 lg:flex-row'>
          <div className='min-w-0 flex-1'>
            <AutomationGraph
              model={model}
              selectedName={selectedName}
              onSelect={(n) => setSelectedName(n.name)}
            />
          </div>
          <aside className='lg:w-[28rem] lg:shrink-0'>
            <div className='sticky top-3 max-h-[calc(100vh-9rem)] overflow-y-auto border border-border bg-card'>
              <StepInspector node={selectedNode} />
            </div>
          </aside>
        </div>
      </TabsContent>

      <TabsContent value='events' className='mt-0'>
        <AgentEventLog events={events} />
      </TabsContent>
    </Tabs>
  );
};
