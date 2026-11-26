import type React from "react";
import { useEffect, useMemo, useState } from "react";
import { toast } from "sonner";
import BackfillRangesPanel from "@/components/airway/BackfillRangesPanel";
import RetryFailedTablesButton from "@/components/airway/RetryFailedTablesButton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import { useStartAirwayRun } from "@/hooks/api/airway/useAirway";
import type { EltTableSummary, RunEventEntry } from "@/services/api/coordinator";
import { AgentEventLog } from "../AgentEventLog";
import { TimeAxis } from "../TimeAxis";
import { buildEltModel, type EltTableNode } from "./model";
import { PipelineHeader } from "./PipelineHeader";
import { EltTableInspector } from "./TableInspector";
import { EltTableRow } from "./TableRow";

/**
 * ELT (airway) run detail body — replaces the flat `EltTablesCard` +
 * generic TaskTree fallback for `source_type = "airway"` runs. Two
 * tabs (Graph default / Events) matching the automation run shape.
 *
 * The Graph tab is the hero: pipeline lineage header at top, per-table
 * cards stacked vertically with tri-banded extract/normalize/load
 * proportional bars on a shared time axis, and a sticky inspector on
 * the right surfacing per-phase row counts, drop% callout, child
 * tables, and schema diff.
 */
export const EltBody: React.FC<{
  tables: EltTableSummary[];
  events: RunEventEntry[];
  /** Lineage labels stamped on the run row at start time. Wired in by
   *  `RunDetail` from the root node; lets the Source / Destination
   *  cards label themselves with real connector names even before the
   *  `pipeline_plan` event fires (and for older runs that predate it). */
  pipelineName?: string | null;
  sourceKind?: string | null;
  destinationLabel?: string | null;
  /** Run-row `error_message`. Pre-flight failures (secret resolution,
   *  spec validation) happen *before* the airway worker starts so they
   *  never emit `pipeline_error` events — the failure is recorded
   *  straight on the run row. Surface it as a banner just like the
   *  IDE pipeline page does. */
  runError?: string | null;
  /** Authoring `.airway.yml` ref (`metadata.pipeline_ref`), needed to
   *  start a "re-run failed tables" run. `null` for runs without a YAML
   *  source — the retry action is hidden then. */
  pipelineRef?: string | null;
}> = ({ tables, events, pipelineName, sourceKind, destinationLabel, runError, pipelineRef }) => {
  const startRun = useStartAirwayRun();
  const model = useMemo(
    () =>
      buildEltModel(tables, events, {
        pipelineName,
        sourceKind,
        destinationLabel,
        runError
      }),
    [tables, events, pipelineName, sourceKind, destinationLabel, runError]
  );
  const [selectedName, setSelectedName] = useState<string | null>(null);

  // Auto-select the first failed table (so debugging starts where it
  // matters) or fall back to the first row.
  useEffect(() => {
    if (selectedName) return;
    const failed = model.tables.find((t) => t.status === "failed");
    const pick = failed ?? model.tables[0];
    if (pick) setSelectedName(pick.name);
  }, [model.tables, selectedName]);

  const selectedTable = useMemo<EltTableNode | null>(
    () => model.tables.find((t) => t.name === selectedName) ?? null,
    [model.tables, selectedName]
  );

  // Which tables had a schema_evolved event mention. Pre-computed once
  // so each row can flash a "schema +" pill without an O(n²) scan.
  const tablesWithSchemaChange = useMemo(() => {
    const set = new Set<string>();
    for (const c of model.schemaChanges) {
      try {
        const json = JSON.stringify(c.changes).toLowerCase();
        for (const t of model.tables) {
          if (json.includes(t.name.toLowerCase())) set.add(t.name);
        }
      } catch {
        // Bad payload — surface as a top-banner change without
        // attributing to a specific table.
      }
    }
    return set;
  }, [model.schemaChanges, model.tables]);

  return (
    <Tabs defaultValue='graph' className='gap-0'>
      <PipelineHeader model={model} />

      <div className='flex items-center justify-between border-border border-b px-4 py-2'>
        <TabsList>
          <TabsTrigger value='graph'>Graph</TabsTrigger>
          <TabsTrigger value='events'>Events</TabsTrigger>
          {pipelineRef && <TabsTrigger value='coverage'>Coverage</TabsTrigger>}
        </TabsList>
        {pipelineRef && (
          <RetryFailedTablesButton
            failedTables={model.tables.filter((t) => t.status === "failed").map((t) => t.name)}
            pending={startRun.isPending}
            onConfirm={async (tablesToRetry) => {
              try {
                await startRun.mutateAsync({ pipeline_ref: pipelineRef, resources: tablesToRetry });
                toast.success("New run started", {
                  description: `Re-running ${tablesToRetry.length} failed table${
                    tablesToRetry.length === 1 ? "" : "s"
                  }. It'll appear in the runs list.`
                });
              } catch (e) {
                toast.error("Couldn't start the new run", {
                  description: e instanceof Error ? e.message : "Please try again."
                });
              }
            }}
          />
        )}
      </div>

      <TabsContent value='graph' className='mt-0'>
        {model.tables.length === 0 ? (
          <div className='px-4 py-10 text-center text-muted-foreground text-sm'>
            No tables captured for this airway run yet.
          </div>
        ) : (
          <div className='flex flex-col gap-4 p-3 lg:flex-row'>
            <div className='flex min-w-0 flex-1 flex-col gap-2'>
              {/* Shared time axis above the bars — same widget the
                  automation Graph view uses, just with column-width slots
                  tuned to the ELT card layout (no fixed left/right
                  gutters here, so a tiny left pad matches the bar's
                  `ml-5` and the right slot is zero). */}
              {model.window && (
                <TimeAxis spanMs={model.window.spanMs} leftSlot='w-5' rightSlot='w-0' />
              )}
              {model.tables.map((table) => (
                <EltTableRow
                  key={table.name}
                  table={table}
                  window={model.window}
                  isSelected={selectedName === table.name}
                  hasSchemaChange={tablesWithSchemaChange.has(table.name)}
                  onClick={() => setSelectedName(table.name)}
                />
              ))}
            </div>
            <aside className='lg:w-[28rem] lg:shrink-0'>
              <div className='sticky top-3 max-h-[calc(100vh-9rem)] overflow-y-auto border border-border bg-card'>
                <EltTableInspector table={selectedTable} schemaChanges={model.schemaChanges} />
              </div>
            </aside>
          </div>
        )}
      </TabsContent>

      <TabsContent value='events' className='mt-0'>
        <AgentEventLog events={events} />
      </TabsContent>

      {/* Pipeline-wide chunked-backfill coverage (same panel as the IDE airway
          page) — surfaced here so job details show what period is loaded vs
          missing + the Resume action, not just this run's tables. */}
      {pipelineRef && (
        <TabsContent value='coverage' className='mt-0'>
          <BackfillRangesPanel pipelineRef={pipelineRef} />
        </TabsContent>
      )}
    </Tabs>
  );
};
