import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useSearchParams } from "react-router-dom";
import { Button } from "@/components/ui/shadcn/button";
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup
} from "@/components/ui/shadcn/resizable";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import { useMetricTree, useTimeDimensions } from "@/hooks/api/useMetricTree";
import type { MetricTree } from "@/types/metricTree";
import { DefinitionPanel } from "./components/DefinitionPanel";
import { MetricTreeGraph } from "./components/MetricTreeGraph";
import { SensitivityPanel } from "./components/SensitivityPanel";
import { ImpactList } from "./scenario/ImpactList";
import { LeverList } from "./scenario/LeverList";
import { ProjectionPanel } from "./scenario/ProjectionPanel";
import { pickTimeDimension, usableTimeDimensions } from "./scenario/pickTimeDimension";
import { ScenarioToolbar } from "./scenario/ScenarioToolbar";
import { decodeScenario, type ScenarioState } from "./scenario/scenarioUrl";
import { buildParams, type Mode } from "./scenario/urlParams";
import { useScenario } from "./scenario/useScenario";
import SimulationPanel from "./simulation/SimulationPanel";

const EMPTY_TREE: MetricTree = { nodes: [], edges: [] };

function decodeMode(params: URLSearchParams): Mode {
  const mode = params.get("mode");
  // Explicit allowlist, not a cast: `?mode=` is user-editable, and an unknown
  // value has to land on a mode that renders rather than on a branch that
  // falls through to a blank panel.
  if (mode === "scenario" || mode === "simulation") return mode;
  return "explore";
}

/**
 * Metric Tree view — the workspace's measures as an interactive graph, with
 * an Explore mode (Definition/Drivers side panel), a Scenario mode (pin levers,
 * see the simulated blast radius), and a Simulation mode (run a declared world
 * and watch the estimate walk toward a truth the model was never told).
 */
export default function MetricTreeView() {
  const { data: tree, isLoading, error } = useMetricTree();
  const [selectedId, setSelectedId] = useState<string | null>(null);

  const [searchParams, setSearchParams] = useSearchParams();
  const [mode, setMode] = useState<Mode>(() => decodeMode(searchParams));
  const [scenarioState, setScenarioState] = useState<ScenarioState>(() =>
    decodeScenario(searchParams)
  );

  // The scenario state as of the last update, not as of this render.
  //
  // Three separate effects write whole-scenario state — the time-dimension
  // adopt below, `useDropVanishedLevers`, and `pinLever` — and each used to
  // close over the same render's `scenarioState`. Two of them committing in
  // one tick meant the second spread a snapshot taken BEFORE the first, so
  // the first's field was silently reverted. Accepting an updater and
  // resolving it against this ref removes the whole class: a caller that
  // passes `prev => ...` always sees the latest write, in the same tick.
  const scenarioRef = useRef(scenarioState);
  scenarioRef.current = scenarioState;

  const updateMode = useCallback(
    (next: Mode) => {
      setMode(next);
      setSearchParams((params) => buildParams(params, next, scenarioRef.current), {
        replace: true
      });
    },
    [setSearchParams]
  );

  const updateScenarioState = useCallback(
    (update: ScenarioState | ((prev: ScenarioState) => ScenarioState)) => {
      const next = typeof update === "function" ? update(scenarioRef.current) : update;
      scenarioRef.current = next;
      setScenarioState(next);
      setSearchParams((params) => buildParams(params, mode, next), { replace: true });
    },
    [mode, setSearchParams]
  );

  const { data: timeDimsResponse } = useTimeDimensions({ enabled: mode === "scenario" });
  const byView = useMemo(() => timeDimsResponse?.by_view ?? {}, [timeDimsResponse]);
  // Only the dimensions that can panel the pinned levers. Offering the whole
  // layer let a `checks` lever be grouped by `store_days.business_date`, which
  // joins the coarser view in on `location_id` alone and flattens every
  // measure across dates — see `usableTimeDimensions`.
  const timeDimensions = useMemo(
    () => usableTimeDimensions(scenarioState.levers, byView),
    [scenarioState.levers, byView]
  );

  // Adopt a time dimension from a lever's own view once one is pinned, so a
  // layer that has one gets a real baseline instead of falling back to
  // delta-only. Never borrows a dimension from an unrelated view — see
  // `pickTimeDimension`; a wrong pick fails the baseline query outright.
  useEffect(() => {
    if (mode !== "scenario" || scenarioState.timeDimension !== null) return;
    const picked = pickTimeDimension(scenarioState.levers, byView);
    if (picked) updateScenarioState((prev) => ({ ...prev, timeDimension: picked }));
  }, [mode, scenarioState, byView, updateScenarioState]);

  // A time dimension no pinned lever's view declares — restored from the URL,
  // or left behind when the lever set changed. The toolbar keeps rendering it
  // and says why, but the QUERY must not carry it: grouping by it joins that
  // view in on whatever key the two share, with nothing tying the dates, so
  // every measure comes back as a flat rectangle — 26,280 rows of constant
  // where the honest answer is a ragged 13,498. Delta-only is strictly better
  // than a fit over a collapsed panel, so the request drops it and the
  // scenario falls back to that.
  // Gated on the dimensions having actually ARRIVED. `timeDimensions` is `[]`
  // while `useTimeDimensions` is in flight and stays `[]` if it fails, so
  // without this every dimension looks foreign: reloading a shared scenario
  // URL that carries `time_dim` would drop it from the request and show the
  // red banner until the fetch landed — and forever, silently delta-only, if
  // it errored.
  const foreignTimeDimension =
    timeDimsResponse !== undefined &&
    scenarioState.timeDimension !== null &&
    !timeDimensions.includes(scenarioState.timeDimension);
  const queryState = useMemo(
    () => (foreignTimeDimension ? { ...scenarioState, timeDimension: null } : scenarioState),
    [foreignTimeDimension, scenarioState]
  );

  // Called unconditionally (rules-of-hooks): cheap on an empty tree, and lets
  // the graph + right panel share one computed result.
  const scenario = useScenario({ tree: tree ?? EMPTY_TREE, state: queryState });

  // Scenario mode with nothing pinned has nothing to say, and its states are
  // all comparisons against a lever that doesn't exist yet: every node falls
  // through to `unreachable` and the whole canvas dims to unreadable — while
  // clicking a node is precisely how the first lever gets pinned. So the
  // canvas stays in its plain form until a lever exists to compare against.
  const showsScenario = mode === "scenario" && scenarioState.levers.length > 0;

  const pinLever = useCallback(
    (nodeId: string) => {
      if (scenarioState.levers.some((l) => l.nodeId === nodeId)) return;
      const baseline = scenario.nodeData.get(nodeId)?.baseline;
      updateScenarioState((prev) => ({
        ...prev,
        levers: [
          ...prev.levers,
          // "+0", not "": a signed zero delta is the one input mode that
          // works with no baseline, and it resolves to `no_change` — the
          // deliberate no-op a freshly pinned lever should start at. Empty
          // resolves to `not_a_number`, so the FIRST lever ever pinned (the
          // baseline query is gated on a lever existing, so there is never a
          // baseline yet) opened with a red error under it.
          { nodeId, raw: baseline !== undefined ? String(baseline) : "+0" }
        ]
      }));
    },
    [scenarioState, scenario.nodeData, updateScenarioState]
  );

  const handleSelect = useCallback(
    (id: string) => {
      setSelectedId(id);
      if (mode === "scenario") pinLever(id);
    },
    [mode, pinLever]
  );

  if (isLoading) {
    return (
      <div className='flex h-full items-center justify-center'>
        <Spinner />
      </div>
    );
  }
  if (error) {
    return (
      <div className='flex h-full items-center justify-center text-destructive text-sm'>
        {error instanceof Error ? error.message : "Failed to load the metric tree."}
      </div>
    );
  }
  if (!tree) return null;

  return (
    <div
      className='flex h-full min-h-0 flex-1 flex-col overflow-hidden'
      data-testid='metric-tree-view'
    >
      {mode === "scenario" && (
        <ScenarioToolbar
          state={scenarioState}
          onChange={updateScenarioState}
          timeDimensions={timeDimensions}
          foreign={foreignTimeDimension ? scenarioState.timeDimension : null}
        />
      )}
      {/* The panel carries a projection chart, a lever list and an impact list
          in one column, so its useful width varies a lot by what the analyst is
          doing. `autoSaveId` persists whatever they settle on across reloads. */}
      <ResizablePanelGroup
        direction='horizontal'
        autoSaveId='metric-tree-panel'
        className='min-h-0 flex-1 overflow-hidden'
      >
        <ResizablePanel minSize={25} className='relative h-full min-w-0'>
          <MetricTreeGraph
            tree={tree}
            selectedId={selectedId}
            onSelect={handleSelect}
            onClearSelection={() => setSelectedId(null)}
            scenario={showsScenario ? scenario.nodeData : undefined}
          />
          {showsScenario && scenario.unreachableCount > 0 && (
            <div className='absolute bottom-3 left-3 z-10 border border-border bg-card/90 px-3 py-1.5 text-muted-foreground text-xs backdrop-blur'>
              {scenario.unreachableCount} measure{scenario.unreachableCount === 1 ? "" : "s"}{" "}
              unaffected
            </div>
          )}
        </ResizablePanel>
        <ResizableHandle withHandle />
        <ResizablePanel
          defaultSize={32}
          minSize={22}
          maxSize={55}
          className='flex min-w-0 flex-col overflow-hidden border-border border-l'
        >
          {mode === "explore" ? (
            <ExplorePanel
              measureId={selectedId}
              tree={tree}
              mode={mode}
              onModeChange={updateMode}
            />
          ) : mode === "simulation" ? (
            <div className='flex min-h-0 flex-1 flex-col gap-0'>
              <div className='flex shrink-0 items-center justify-between gap-2 border-border border-b bg-background px-3 py-2'>
                <span className='min-w-0 flex-1 truncate font-mono text-[10px] text-foreground uppercase tracking-wider'>
                  Simulation
                </span>
                <ModeToggle mode={mode} onChange={updateMode} />
              </div>
              <div className='min-h-0 flex-1 overflow-auto'>
                <SimulationPanel />
              </div>
            </div>
          ) : (
            <div className='flex min-h-0 flex-1 flex-col gap-0'>
              <div className='flex shrink-0 items-center justify-between gap-2 border-border border-b bg-background px-3 py-2'>
                <span className='min-w-0 flex-1 truncate font-mono text-[10px] text-foreground uppercase tracking-wider'>
                  Scenario
                </span>
                <ModeToggle mode={mode} onChange={updateMode} />
              </div>
              <div className='min-h-0 flex-1 overflow-auto'>
                <LeverList
                  tree={tree}
                  state={scenarioState}
                  onChange={updateScenarioState}
                  conflicts={scenario.conflicts}
                  leverErrors={scenario.leverErrors}
                  requestError={scenario.propagationError ?? scenario.baselineError}
                  baselineNote={scenario.baselineNote}
                  anyValued={scenario.anyValued}
                  nodeData={scenario.nodeData}
                  fitted={scenario.fitted}
                />
                {scenarioState.levers.length > 0 && (
                  <>
                    <ImpactList
                      nodeData={scenario.nodeData}
                      onSelect={setSelectedId}
                      runState={scenario.runState}
                      tree={tree}
                      fitted={scenario.fitted}
                      leverIds={scenarioState.levers.map((l) => l.nodeId)}
                    />
                    <ProjectionPanel
                      state={queryState}
                      nodeData={scenario.nodeData}
                      selectedId={selectedId}
                      blocked={scenario.conflicts.length > 0}
                    />
                  </>
                )}
              </div>
            </div>
          )}
        </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  );
}

function ModeToggle({ mode, onChange }: { mode: Mode; onChange: (next: Mode) => void }) {
  return (
    <div className='flex items-center gap-0.5 border border-border p-0.5'>
      <Button
        type='button'
        variant={mode === "explore" ? "secondary" : "ghost"}
        size='sm'
        className='h-6 px-2 text-xs'
        onClick={() => onChange("explore")}
        data-testid='metric-tree-mode-explore'
      >
        Explore
      </Button>
      <Button
        type='button'
        variant={mode === "scenario" ? "secondary" : "ghost"}
        size='sm'
        className='h-6 px-2 text-xs'
        onClick={() => onChange("scenario")}
        data-testid='metric-tree-mode-scenario'
      >
        Scenario
      </Button>
      <Button
        type='button'
        variant={mode === "simulation" ? "secondary" : "ghost"}
        size='sm'
        className='h-6 px-2 text-xs'
        onClick={() => onChange("simulation")}
        data-testid='metric-tree-mode-simulation'
      >
        Simulation
      </Button>
    </div>
  );
}

interface ExplorePanelProps {
  measureId: string | null;
  tree: MetricTree;
  mode: Mode;
  onModeChange: (next: Mode) => void;
}

/** Explore mode's Definition/Drivers tabs, with the mode toggle beside the
 *  TabsList in the same header row. One `Tabs` root so the trigger and its
 *  content stay wired together. */
function ExplorePanel({ measureId, tree, mode, onModeChange }: ExplorePanelProps) {
  return (
    <Tabs defaultValue='definition' className='flex min-h-0 flex-1 flex-col gap-0'>
      <div className='flex items-center justify-between gap-2 border-border border-b px-4 py-2'>
        <TabsList className='w-fit'>
          <TabsTrigger value='definition'>Definition</TabsTrigger>
          <TabsTrigger value='drivers'>Drivers</TabsTrigger>
        </TabsList>
        <ModeToggle mode={mode} onChange={onModeChange} />
      </div>
      <TabsContent value='definition' className='min-h-0 flex-1 overflow-auto'>
        <DefinitionPanel measureId={measureId} tree={tree} />
      </TabsContent>
      <TabsContent value='drivers' className='min-h-0 flex-1 overflow-auto'>
        <SensitivityPanel measureId={measureId} />
      </TabsContent>
    </Tabs>
  );
}
