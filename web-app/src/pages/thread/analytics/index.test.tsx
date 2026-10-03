// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SidebarProvider } from "@/components/ui/shadcn/sidebar";
import type useBuilderAvailable from "@/hooks/api/useBuilderAvailable";
import type { SseEvent, UseAnalyticsRunResult } from "@/hooks/useAnalyticsRun";
import type { AnalyticsRunSummary, ChartConfig } from "@/services/api/analytics";
import { setPendingThinkingMode } from "@/stores/analyticsThinkingMode";
import type { ThreadItem } from "@/types/chat";
import AnalyticsThread from "./index";

// The reasoning trace and the artifact sidebar are rendered for real: what these
// tests pin is the page wiring between them (an item picked in the trace opens the
// right panel, fed by the right run's events). Only the edges are replaced — the
// run state machine, the run-list query, and leaves that need a browser.

type RunsQuery = { data: AnalyticsRunSummary[]; isLoading: boolean; isFetching: boolean };
type BuilderAvailability = ReturnType<typeof useBuilderAvailable>;

// ── Module mocks ──────────────────────────────────────────────────────────────

const { mockUseAnalyticsRun, mockUseQuery, mockUseBuilderAvailable, queryClientStub } = vi.hoisted(
  () => ({
    mockUseAnalyticsRun: vi.fn<() => UseAnalyticsRunResult>(),
    mockUseQuery: vi.fn<() => RunsQuery>(),
    mockUseBuilderAvailable: vi.fn<() => BuilderAvailability>(),
    // One stable client, as in the app: a fresh object per render would re-fire
    // every effect that lists the client as a dependency.
    queryClientStub: { invalidateQueries: vi.fn(), refetchQueries: vi.fn() }
  })
);

vi.mock("@tanstack/react-query", async (importOriginal) => {
  const original = await importOriginal<typeof import("@tanstack/react-query")>();
  return {
    ...original,
    useQuery: (options: { queryKey: readonly unknown[] }) => {
      // Only the thread's run list is served here. A query added to this tree later
      // must be mocked on purpose rather than silently handed a list of runs.
      if (options.queryKey[0] !== "analytics" || options.queryKey[1] !== "runsByThread") {
        throw new Error(`unmocked query: ${JSON.stringify(options.queryKey)}`);
      }
      return mockUseQuery();
    },
    useQueryClient: () => queryClientStub
  };
});

vi.mock("@/hooks/useAnalyticsRun", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/hooks/useAnalyticsRun")>();
  return { ...original, useAnalyticsRun: mockUseAnalyticsRun };
});

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));

vi.mock("@/hooks/api/useBuilderAvailable", () => ({ default: mockUseBuilderAvailable }));

// Read by the builder input for @-mentions; it needs a QueryClient of its own.
vi.mock("@/hooks/api/files/useFileTree", () => ({ default: () => ({ data: undefined }) }));

// lottie-web paints a canvas at import time and jsdom has no canvas.
vi.mock("@lottiefiles/react-lottie-player", () => ({ Player: "div" }));

vi.mock("@/components/ui/shadcn/resizable", () => ({
  ResizablePanelGroup: ({ children }: { children: ReactNode }) => (
    <div data-testid='panel-group'>{children}</div>
  ),
  ResizablePanel: ({ children }: { children: ReactNode }) => <div>{children}</div>,
  ResizableHandle: () => null
}));

vi.mock("./Header", () => ({ default: () => <div data-testid='thread-header' /> }));
vi.mock("./SuspensionPrompt", () => ({ default: () => null }));
vi.mock("@/components/Messages/UserMessage", () => ({
  default: ({ content }: { content: string }) => <div data-testid='user-message'>{content}</div>
}));
vi.mock("@/components/Markdown", () => ({
  default: ({ children }: { children: string }) => <div>{children}</div>
}));
// Charts draw through DuckDB WASM and ECharts. Stand in with a marker that says
// which chart was asked for, so a test can tell one chart from another.
vi.mock("@/components/AppPreview/Displays", () => ({
  DisplayBlock: ({ display }: { display: { type: string; title?: string } }) => (
    <div data-testid='display-block' data-chart-type={display.type}>
      {display.title}
    </div>
  )
}));

// ── Fixtures ──────────────────────────────────────────────────────────────────

const THREAD: ThreadItem = {
  id: "thread-1",
  source: "agent-1",
  input: "Analyze sales data",
  title: "Sales Analysis",
  output: "",
  source_type: "analytics",
  created_at: "2026-01-01T00:00:00Z",
  references: [],
  is_processing: false
};

const BUILDER_THREAD: ThreadItem = { ...THREAD, source: "__builder__" };

let counter = 0;

const sseEv = <T extends SseEvent["type"]>(
  type: T,
  data: Extract<SseEvent, { type: T }>["data"]
): SseEvent => ({ id: String(counter++), type, data }) as SseEvent;

type AutomationStep = { name: string; task_type: string };

/**
 * The events the pipeline emits when a step hands off to an automation: the step
 * suspends on a delegation prompt and stays open, and the automation attaches to it.
 */
const automationStarted = (name: string, steps: AutomationStep[]): SseEvent[] => [
  sseEv("step_start", { label: "Executing" }),
  sseEv("awaiting_input", {
    questions: [{ prompt: `Executing step: ${name}`, suggestions: [] }]
  }),
  sseEv("step_end", { label: "Executing", outcome: "suspended" }),
  sseEv("subrun_started", { subrun_name: name, steps })
];

/** A `render_chart` tool call followed by the chart it produced. */
const chartRendered = (config: ChartConfig, rows: unknown[][]) => {
  const block = { config, columns: [config.x ?? "x", config.y ?? "y"], rows };
  return {
    block,
    events: [
      sseEv("tool_call", { name: "render_chart", input: config }),
      sseEv("chart_rendered", block)
    ]
  };
};

const pastRun = (overrides: Partial<AnalyticsRunSummary> = {}): AnalyticsRunSummary => ({
  run_id: "r1",
  agent_id: "agent-1",
  question: "Analyze sales data",
  status: "done",
  ui_events: [],
  ...overrides
});

const noop = () => {};

function makeResult(overrides: Partial<UseAnalyticsRunResult> = {}): UseAnalyticsRunResult {
  return {
    state: { tag: "idle" },
    start: noop,
    reconnect: noop,
    hydrate: noop,
    answer: noop,
    stop: noop,
    reset: noop,
    isStarting: false,
    isAnswering: false,
    ...overrides
  };
}

function runningWith(events: SseEvent[]): UseAnalyticsRunResult {
  return makeResult({ state: { tag: "running", runId: "run-1", events } });
}

function doneWith(
  events: SseEvent[],
  displayBlocks: { config: ChartConfig; columns: string[]; rows: unknown[][] }[] = []
): UseAnalyticsRunResult {
  return makeResult({
    state: { tag: "done", runId: "run-1", answer: "", displayBlocks, durationMs: 0, events }
  });
}

const builderAvailability = (
  overrides: Partial<BuilderAvailability> = {}
): BuilderAvailability => ({
  isAvailable: true,
  isLoading: false,
  isError: false,
  builderPath: "",
  isBuiltin: true,
  builderModel: undefined,
  ...overrides
});

const renderThread = (thread: ThreadItem = THREAD) =>
  render(<AnalyticsThread thread={thread} />, { wrapper: SidebarProvider });

/** The pill the reasoning trace shows for an automation or a tool call. */
const pills = (label: string) =>
  screen.getAllByTestId(`reasoning-pill-${label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}`);

const selectInTrace = (label: string, nth = 0) => fireEvent.click(pills(label)[nth]);

/** Queries scoped to the side panel whose header carries `title`. */
const panel = (title: string) => {
  const root = screen
    .getByRole("heading", { name: title })
    .closest<HTMLElement>("[data-slot='panel']");
  if (!root) throw new Error(`"${title}" heading is not inside a panel`);
  return within(root);
};

beforeEach(() => {
  counter = 0;
  mockUseAnalyticsRun.mockReturnValue(makeResult());
  mockUseQuery.mockReturnValue({ data: [], isLoading: false, isFetching: false });
  mockUseBuilderAvailable.mockReturnValue(builderAvailability());
  // jsdom does not implement scrollIntoView
  window.HTMLElement.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

// ── Automation run panel ──────────────────────────────────────────────────────

describe("AnalyticsThread — automation run panel", () => {
  const STEPS: AutomationStep[] = [
    { name: "fetch_data", task_type: "execute_sql" },
    { name: "process_data", task_type: "execute_sql" },
    { name: "generate_report", task_type: "formatter" }
  ];

  it("shows no panel while idle", () => {
    renderThread();
    expect(screen.queryByRole("heading")).toBeNull();
  });

  it("stays closed until the automation is picked in the trace", () => {
    mockUseAnalyticsRun.mockReturnValue(runningWith(automationStarted("store_deep_dive", STEPS)));
    renderThread();
    expect(pills("store_deep_dive")).toHaveLength(1);
    expect(screen.queryByRole("heading")).toBeNull();
  });

  it("opens the picked automation's run with its name and steps", () => {
    mockUseAnalyticsRun.mockReturnValue(runningWith(automationStarted("store_deep_dive", STEPS)));
    renderThread();
    selectInTrace("store_deep_dive");
    const run = panel("store_deep_dive");
    for (const step of STEPS) {
      expect(run.getByText(step.name)).toBeInTheDocument();
    }
  });

  it("opens the run of whichever automation was picked, not the latest one", () => {
    mockUseAnalyticsRun.mockReturnValue(
      runningWith([
        ...automationStarted("first_proc", [{ name: "step_a", task_type: "execute_sql" }]),
        sseEv("subrun_started", {
          subrun_name: "second_proc",
          steps: [{ name: "step_b", task_type: "execute_sql" }]
        })
      ])
    );
    renderThread();

    selectInTrace("first_proc");
    expect(panel("first_proc").getByText("step_a")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "second_proc" })).toBeNull();

    selectInTrace("second_proc");
    expect(panel("second_proc").getByText("step_b")).toBeInTheDocument();
    expect(screen.queryByRole("heading", { name: "first_proc" })).toBeNull();
  });

  it("keeps step status live as events arrive after the panel was opened", () => {
    const started = automationStarted("p", [{ name: "step_a", task_type: "execute_sql" }]);
    mockUseAnalyticsRun.mockReturnValue(runningWith(started));
    const { rerender } = renderThread();
    selectInTrace("p");
    // The header says the run is going; the step itself has not started.
    expect(panel("p").getAllByText("Running…")).toHaveLength(1);

    const stepRunning = [...started, sseEv("subrun_step_started", { step: "step_a" })];
    mockUseAnalyticsRun.mockReturnValue(runningWith(stepRunning));
    rerender(<AnalyticsThread thread={THREAD} />);
    expect(panel("p").getAllByText("Running…")).toHaveLength(2);

    mockUseAnalyticsRun.mockReturnValue(
      runningWith([
        ...stepRunning,
        sseEv("subrun_step_completed", { step: "step_a", success: true })
      ])
    );
    rerender(<AnalyticsThread thread={THREAD} />);
    expect(panel("p").getByText("Done")).toBeInTheDocument();
    expect(panel("p").getAllByText("Running…")).toHaveLength(1);
  });

  it("shows a step as Failed when it completes unsuccessfully", () => {
    mockUseAnalyticsRun.mockReturnValue(
      runningWith([
        ...automationStarted("p", [{ name: "step_a", task_type: "execute_sql" }]),
        sseEv("subrun_step_started", { step: "step_a" }),
        sseEv("subrun_step_completed", { step: "step_a", success: false, error: "timeout" })
      ])
    );
    renderThread();
    selectInTrace("p");
    expect(panel("p").getByText("Failed")).toBeInTheDocument();
  });

  it("says Running… in the header while the run is streaming", () => {
    mockUseAnalyticsRun.mockReturnValue(
      runningWith(automationStarted("running_proc", [{ name: "step_a", task_type: "execute_sql" }]))
    );
    renderThread();
    selectInTrace("running_proc");
    const header = screen
      .getByRole("heading", { name: "running_proc" })
      .closest<HTMLElement>("[data-slot='panel-header']");
    expect(header).not.toBeNull();
    expect(header?.textContent).toContain("Running…");
  });

  it("says Completed in the header once the automation succeeded and the run is over", () => {
    mockUseAnalyticsRun.mockReturnValue(
      doneWith([
        ...automationStarted("p", [{ name: "step_a", task_type: "execute_sql" }]),
        sseEv("subrun_step_completed", { step: "step_a", success: true }),
        sseEv("subrun_completed", { subrun_name: "p", success: true }),
        sseEv("input_resolved", { answer: "done" })
      ])
    );
    renderThread();
    selectInTrace("p");
    const header = screen
      .getByRole("heading", { name: "p" })
      .closest<HTMLElement>("[data-slot='panel-header']");
    expect(header?.textContent).toContain("Completed");
    expect(header?.textContent).not.toContain("Running…");
  });

  it("closes from the panel's close button and stays closed on the next render", () => {
    mockUseAnalyticsRun.mockReturnValue(
      runningWith(automationStarted("my_proc", [{ name: "step_a", task_type: "execute_sql" }]))
    );
    const { rerender } = renderThread();

    selectInTrace("my_proc");
    expect(screen.getByRole("heading", { name: "my_proc" })).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Close panel" }));
    expect(screen.queryByRole("heading", { name: "my_proc" })).toBeNull();

    rerender(<AnalyticsThread thread={THREAD} />);
    expect(screen.queryByRole("heading", { name: "my_proc" })).toBeNull();
  });
});

// ── Thread switching ──────────────────────────────────────────────────────────

describe("AnalyticsThread — thread switching", () => {
  const THREAD_2: ThreadItem = { ...THREAD, id: "thread-2" };

  it("calls reset when thread.id changes while a run is active", () => {
    const reset = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(
      makeResult({ state: { tag: "running", runId: "run-1", events: [] }, reset })
    );
    const { rerender } = renderThread();
    // Mounting resets too; only the switch is under test.
    reset.mockClear();

    rerender(<AnalyticsThread thread={THREAD_2} />);
    expect(reset).toHaveBeenCalledTimes(1);
  });

  it("calls reset when thread.id changes while state is idle", () => {
    const reset = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ reset }));
    const { rerender } = renderThread();
    reset.mockClear();

    rerender(<AnalyticsThread thread={THREAD_2} />);
    expect(reset).toHaveBeenCalledTimes(1);
  });

  it("does not call reset on re-render with the same thread.id", () => {
    const reset = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ reset }));
    const { rerender } = renderThread();
    reset.mockClear();

    rerender(<AnalyticsThread thread={{ ...THREAD }} />);
    expect(reset).not.toHaveBeenCalled();
  });

  it("closes the open panel when the thread changes", () => {
    mockUseAnalyticsRun.mockReturnValue(
      runningWith(automationStarted("my_proc", [{ name: "step_a", task_type: "execute_sql" }]))
    );
    const { rerender } = renderThread();
    selectInTrace("my_proc");
    expect(screen.getByRole("heading", { name: "my_proc" })).toBeInTheDocument();

    rerender(<AnalyticsThread thread={THREAD_2} />);
    expect(screen.queryByRole("heading", { name: "my_proc" })).toBeNull();
  });
});

// ── Auto-start on first visit ─────────────────────────────────────────────────

describe("AnalyticsThread — auto-start on first visit", () => {
  it("starts the thread's question once when the thread has no run yet", () => {
    const start = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ start }));
    renderThread();
    expect(start).toHaveBeenCalledTimes(1);
    expect(start).toHaveBeenCalledWith(
      "agent-1",
      "Analyze sales data",
      "thread-1",
      "auto",
      undefined
    );
  });

  it("starts with the thinking mode chosen in the chat panel", () => {
    const start = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ start }));
    setPendingThinkingMode("thread-1", "extended_thinking");
    renderThread();
    expect(start).toHaveBeenCalledWith(
      "agent-1",
      "Analyze sales data",
      "thread-1",
      "extended_thinking",
      undefined
    );
  });

  it("shows the question and a running trace while the first run is starting", () => {
    renderThread();
    expect(screen.getByTestId("user-message").textContent).toBe("Analyze sales data");
    expect(screen.getByText("Reasoning trace")).toBeInTheDocument();
  });

  it("does not auto-start when allRuns exist (not first visit)", () => {
    const start = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ start }));
    mockUseQuery.mockReturnValue({
      data: [pastRun({ question: "q" })],
      isLoading: false,
      isFetching: false
    });
    renderThread();
    expect(start).not.toHaveBeenCalled();
  });

  it("does not auto-start while allRuns are still loading", () => {
    const start = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ start }));
    mockUseQuery.mockReturnValue({ data: [], isLoading: true, isFetching: true });
    renderThread();
    expect(start).not.toHaveBeenCalled();
  });

  it("holds a builder thread until the builder model is known", () => {
    const start = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ start }));
    mockUseBuilderAvailable.mockReturnValue(builderAvailability({ isLoading: true }));
    renderThread(BUILDER_THREAD);
    expect(start).not.toHaveBeenCalled();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("starts a builder thread with the configured builder model", () => {
    const start = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ start }));
    mockUseBuilderAvailable.mockReturnValue(builderAvailability({ builderModel: "builder-model" }));
    renderThread(BUILDER_THREAD);
    expect(start).toHaveBeenCalledTimes(1);
    expect(start).toHaveBeenCalledWith(
      "__builder__",
      "Analyze sales data",
      "thread-1",
      "auto",
      "builder-model"
    );
  });

  it("says why a builder thread cannot start when no model is configured", () => {
    const start = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ start }));
    renderThread(BUILDER_THREAD);
    expect(start).not.toHaveBeenCalled();
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("The Builder Agent didn't start");
    expect(alert.textContent).toContain("Set builder_agent.model in config.yml");
  });

  it("blames the availability check, not the config, when that check failed", () => {
    mockUseBuilderAvailable.mockReturnValue(builderAvailability({ isError: true }));
    renderThread(BUILDER_THREAD);
    const alert = screen.getByRole("alert");
    expect(alert.textContent).toContain("Couldn't check this workspace's Builder Agent");
    expect(alert.textContent).not.toContain("config.yml");
  });
});

// ── Race condition: stale-cache duplicate auto-start ─────────────────────────
//
// Scenario (key={threadId} means full remount on every thread switch):
//   1. Visit thread-1 → allRuns fetches, returns [], auto-start fires, run-1 begins.
//   2. Navigate to thread-2 → full remount, allRuns fetches, returns [], auto-start fires.
//   3. Switch back to thread-1 (run-1 still in-flight, never finished).
//   4. Switch back to thread-2.
//      → React Query has stale cache [] for thread-2 (run-2 was created AFTER the cache
//        was last written, so it is not reflected).
//      → The component remounts fresh.
//      → isFetching=true (background refetch in progress), but isLoading=false.
//      → Without the fix: isFirstVisit = !isLoading && [] && idle = true → DUPLICATE start.
//      → With the fix: isFirstVisit must also require !isFetching → no start until
//        the refetch resolves and allRuns is confirmed non-empty.

describe("AnalyticsThread — stale-cache duplicate auto-start race condition", () => {
  it("does not auto-start when allRuns cache is empty but a background fetch is in progress", () => {
    // Simulate returning to a thread: stale cache is empty, but isFetching=true
    // (the background refetch hasn't completed yet).
    const start = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ start }));
    mockUseQuery.mockReturnValue({ data: [], isLoading: false, isFetching: true });
    renderThread();
    // Must NOT auto-start while a fetch is still in progress — allRuns may return data.
    expect(start).not.toHaveBeenCalled();
  });

  it("auto-starts only after the fetch completes and allRuns is confirmed empty", () => {
    const start = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ start }));
    mockUseQuery.mockReturnValue({ data: [], isLoading: false, isFetching: true });
    const { rerender } = renderThread();
    expect(start).not.toHaveBeenCalled();

    // Fetch complete, truly empty → genuine first visit
    mockUseQuery.mockReturnValue({ data: [], isLoading: false, isFetching: false });
    rerender(<AnalyticsThread thread={THREAD} />);
    expect(start).toHaveBeenCalledTimes(1);
  });

  it("resumes the in-flight run instead of starting a second one when the fetch finds it", () => {
    const start = vi.fn();
    const reconnect = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ start, reconnect }));
    mockUseQuery.mockReturnValue({ data: [], isLoading: false, isFetching: true });
    const { rerender } = renderThread();

    mockUseQuery.mockReturnValue({
      data: [pastRun({ status: "running" })],
      isLoading: false,
      isFetching: false
    });
    rerender(<AnalyticsThread thread={THREAD} />);
    expect(start).not.toHaveBeenCalled();
    expect(reconnect).toHaveBeenCalledWith("r1", "running");
  });

  it("does not render duplicate questions when allRuns contains the original run", () => {
    mockUseQuery.mockReturnValue({
      data: [pastRun({ answer: "Result" })],
      isLoading: false,
      isFetching: false
    });
    renderThread();
    const messages = screen.getAllByTestId("user-message");
    expect(messages).toHaveLength(1);
    expect(messages[0].textContent).toBe("Analyze sales data");
  });

  it("renders a finished run once while it is both live and already in allRuns", () => {
    // The window between the run finishing and the page dropping its live copy:
    // the refetched list already has the run the page is still showing.
    mockUseAnalyticsRun.mockReturnValue(doneWith([]));
    mockUseQuery.mockReturnValue({
      data: [pastRun({ run_id: "run-1", answer: "Result" })],
      isLoading: false,
      isFetching: false
    });
    renderThread();
    expect(screen.getAllByTestId("user-message")).toHaveLength(1);
  });
});

// ── Chart panel ───────────────────────────────────────────────────────────────

describe("AnalyticsThread — chart panel", () => {
  const BAR: ChartConfig = { chart_type: "bar_chart", x: "month", y: "revenue", title: "Revenue" };
  const LINE: ChartConfig = { chart_type: "line_chart", x: "date", y: "sales", title: "Sales" };

  it("shows the chart settings but no chart while the call has rendered nothing yet", () => {
    mockUseAnalyticsRun.mockReturnValue(
      runningWith([
        sseEv("step_start", { label: "Interpreting" }),
        sseEv("tool_call", { name: "render_chart", input: BAR })
      ])
    );
    renderThread();
    selectInTrace("Render Chart");
    expect(panel("Render Chart").getByText("bar_chart")).toBeInTheDocument();
    expect(screen.queryByTestId("display-block")).toBeNull();
  });

  it("shows the chart rendered by the picked call while the run is still streaming", () => {
    const chart = chartRendered(BAR, [
      ["Jan", 100],
      ["Feb", 200]
    ]);
    mockUseAnalyticsRun.mockReturnValue(
      runningWith([sseEv("step_start", { label: "Interpreting" }), ...chart.events])
    );
    renderThread();
    selectInTrace("Render Chart");

    const rendered = panel("Render Chart").getByTestId("display-block");
    expect(rendered.dataset.chartType).toBe("bar_chart");
    expect(rendered.textContent).toBe("Revenue");
  });

  it("shows each render_chart call its own chart", () => {
    const first = chartRendered(BAR, [["Jan", 100]]);
    const second = chartRendered(LINE, [["2024-01", 500]]);
    mockUseAnalyticsRun.mockReturnValue(
      runningWith([
        sseEv("step_start", { label: "Interpreting" }),
        ...first.events,
        ...second.events
      ])
    );
    renderThread();

    selectInTrace("Render Chart", 1);
    const shown = panel("Render Chart").getAllByTestId("display-block");
    expect(shown).toHaveLength(1);
    expect(shown[0].textContent).toBe("Sales");

    selectInTrace("Render Chart", 0);
    expect(panel("Render Chart").getByTestId("display-block").textContent).toBe("Revenue");
  });

  it("shows the chart for a finished run", () => {
    const chart = chartRendered(LINE, [["2024-01", 500]]);
    mockUseAnalyticsRun.mockReturnValue(
      doneWith(
        [
          sseEv("step_start", { label: "Interpreting" }),
          ...chart.events,
          sseEv("step_end", { label: "Interpreting", outcome: "advanced" })
        ],
        [chart.block]
      )
    );
    renderThread();
    selectInTrace("Render Chart");

    const rendered = panel("Render Chart").getByTestId("display-block");
    expect(rendered.dataset.chartType).toBe("line_chart");
    expect(rendered.textContent).toBe("Sales");
  });
});
