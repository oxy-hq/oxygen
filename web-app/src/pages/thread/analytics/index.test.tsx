// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SidebarProvider } from "@/components/ui/shadcn/sidebar";
import type useBuilderAvailable from "@/hooks/api/useBuilderAvailable";
import type { SseEvent, UseAnalyticsRunResult } from "@/hooks/useAnalyticsRun";
import { sseEventToUiBlock } from "@/hooks/useAnalyticsRun";
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
// The file preview mounts the IDE editors and the delegation panel fetches the child
// run; stand in with what each was opened for.
vi.mock("./FilePreviewPanel", () => ({
  default: ({ change }: { change: { filePath: string } }) => (
    <div data-testid='file-preview'>{change.filePath}</div>
  )
}));
vi.mock("./BuilderDelegationPanel", () => ({
  default: ({ childRunId }: { childRunId: string }) => (
    <div data-testid='delegation-panel'>{childRunId}</div>
  )
}));
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

  it("does not show a finished run's automation as running while another run streams", () => {
    const pastEvents = [
      ...automationStarted("past_proc", [{ name: "step_a", task_type: "execute_sql" }]),
      sseEv("subrun_step_completed", { step: "step_a", success: true }),
      sseEv("subrun_completed", { subrun_name: "past_proc", success: true }),
      sseEv("input_resolved", { answer: "done" })
    ];
    mockUseQuery.mockReturnValue({
      data: [pastRun({ ui_events: pastEvents.map(sseEventToUiBlock) })],
      isLoading: false,
      isFetching: false
    });
    mockUseAnalyticsRun.mockReturnValue(runningWith([sseEv("step_start", { label: "Solving" })]));
    renderThread();
    selectInTrace("past_proc");
    const run = panel("past_proc");
    expect(run.getByText("Done")).toBeInTheDocument();
    expect(run.queryByText("Running…")).toBeNull();
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

// ── Opening a thread ──────────────────────────────────────────────────────────
//
// The thread page keys AnalyticsThread by thread id (pages/thread/index.tsx), so
// another thread is a fresh mount, never a prop change on a mounted one.

describe("AnalyticsThread — opening a thread", () => {
  const THREAD_2: ThreadItem = { ...THREAD, id: "thread-2" };

  it("resumes a suspended run without tearing down the stream it just opened", () => {
    const reconnect = vi.fn();
    const reset = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ reconnect, reset }));
    mockUseQuery.mockReturnValue({
      data: [pastRun({ status: "suspended" })],
      isLoading: false,
      isFetching: false
    });
    renderThread();
    expect(reconnect).toHaveBeenCalledWith("r1", "suspended");
    // A reset aborts that stream, and nothing reopens it while the cached run list
    // stays the same — the suspension prompt would never appear.
    expect(reset).not.toHaveBeenCalled();
  });

  it("does not reset the run on a re-render", () => {
    const reset = vi.fn();
    mockUseAnalyticsRun.mockReturnValue(makeResult({ reset }));
    const { rerender } = renderThread();
    rerender(<AnalyticsThread thread={{ ...THREAD }} />);
    expect(reset).not.toHaveBeenCalled();
  });

  it("opens another thread with no panel open", () => {
    mockUseAnalyticsRun.mockReturnValue(
      runningWith(automationStarted("my_proc", [{ name: "step_a", task_type: "execute_sql" }]))
    );
    const { rerender } = render(<AnalyticsThread key={THREAD.id} thread={THREAD} />, {
      wrapper: SidebarProvider
    });
    selectInTrace("my_proc");
    expect(screen.getByRole("heading", { name: "my_proc" })).toBeInTheDocument();

    rerender(<AnalyticsThread key={THREAD_2.id} thread={THREAD_2} />);
    expect(screen.queryByRole("heading", { name: "my_proc" })).toBeNull();
  });
});

// ── Re-clicking an open pill ──────────────────────────────────────────────────

/** A finished builder run that wrote one file, so the run shows that file's pill. */
const pastBuilderRunWriting = (filePath: string): AnalyticsRunSummary =>
  pastRun({
    agent_id: "__builder__",
    ui_events: [
      sseEventToUiBlock(
        sseEv("file_changed", {
          file_path: filePath,
          description: "",
          new_content: "",
          old_content: "",
          is_deletion: false
        })
      )
    ]
  });

describe("AnalyticsThread — re-clicking an open pill closes it", () => {
  const step = (name: string) => [{ name, task_type: "execute_sql" }];

  it("closes an artifact's panel when its trace pill is clicked again", () => {
    mockUseAnalyticsRun.mockReturnValue(runningWith(automationStarted("my_proc", step("a"))));
    renderThread();
    selectInTrace("my_proc");
    expect(screen.getByRole("heading", { name: "my_proc" })).toBeInTheDocument();

    selectInTrace("my_proc");
    expect(screen.queryByRole("heading", { name: "my_proc" })).toBeNull();
  });

  it("closes a file preview when its file-change pill is clicked again", () => {
    mockUseQuery.mockReturnValue({
      data: [pastBuilderRunWriting("models/orders.view.yml")],
      isLoading: false,
      isFetching: false
    });
    renderThread(BUILDER_THREAD);
    const filePill = screen.getByRole("button", { name: "orders" });
    fireEvent.click(filePill);
    expect(screen.getByTestId("file-preview").textContent).toBe("models/orders.view.yml");

    fireEvent.click(filePill);
    expect(screen.queryByTestId("file-preview")).toBeNull();
  });

  it("closes a builder delegation's panel when its pill is clicked again", () => {
    mockUseAnalyticsRun.mockReturnValue(
      runningWith([
        sseEv("step_start", { label: "Solving" }),
        sseEv("delegation_started", {
          child_task_id: "child-1",
          target: "agent:__builder__",
          request: "add a view"
        })
      ])
    );
    renderThread();
    selectInTrace("Building");
    expect(screen.getByTestId("delegation-panel").textContent).toBe("child-1");

    selectInTrace("Building");
    expect(screen.queryByTestId("delegation-panel")).toBeNull();
  });

  it("switches runs, not closes, when another run's pill sits at the same place", () => {
    // Trace item ids restart at 0 in every run, so two runs with the same shape
    // produce pills with the same id.
    const pastEvents = automationStarted("my_proc", step("past_step")).map(sseEventToUiBlock);
    counter = 0;
    mockUseQuery.mockReturnValue({
      data: [pastRun({ ui_events: pastEvents })],
      isLoading: false,
      isFetching: false
    });
    mockUseAnalyticsRun.mockReturnValue(
      runningWith(automationStarted("my_proc", step("live_step")))
    );
    renderThread();

    selectInTrace("my_proc", 0);
    expect(panel("my_proc").getByText("past_step")).toBeInTheDocument();

    selectInTrace("my_proc", 1);
    expect(panel("my_proc").getByText("live_step")).toBeInTheDocument();
  });

  it("brings back an artifact the builder panel covered instead of closing it", () => {
    const events = [
      sseEv("step_start", { label: "Solving" }),
      sseEv("tool_call", { name: "render_chart", input: { chart_type: "bar_chart" } })
    ];
    mockUseAnalyticsRun.mockReturnValue(runningWith(events));
    const { rerender } = renderThread(BUILDER_THREAD);
    selectInTrace("Render Chart");
    expect(screen.getByRole("heading", { name: "Render Chart" })).toBeInTheDocument();

    // A proposed file change opens the builder panel over the artifact.
    const fileChange = JSON.stringify({ type: "edit_file", file_path: "a.view.yml" });
    mockUseAnalyticsRun.mockReturnValue(
      makeResult({
        state: {
          tag: "suspended",
          runId: "run-1",
          events,
          questions: [{ prompt: fileChange, suggestions: [] }]
        }
      })
    );
    rerender(<AnalyticsThread thread={BUILDER_THREAD} />);
    expect(screen.queryByRole("heading", { name: "Render Chart" })).toBeNull();
    // Selected but not on screen reads as not pressed.
    expect(pills("Render Chart")[0]).toHaveAttribute("aria-pressed", "false");

    selectInTrace("Render Chart");
    expect(screen.getByRole("heading", { name: "Render Chart" })).toBeInTheDocument();
    expect(pills("Render Chart")[0]).toHaveAttribute("aria-pressed", "true");
  });
});

// ── Pressed state ─────────────────────────────────────────────────────────────

describe("AnalyticsThread — a pill says whether its panel is open", () => {
  it("presses the trace pill of the open artifact, and only that one", () => {
    mockUseAnalyticsRun.mockReturnValue(
      runningWith([
        sseEv("step_start", { label: "Interpreting" }),
        sseEv("tool_call", { name: "render_chart", input: { chart_type: "bar_chart" } }),
        sseEv("tool_call", { name: "render_chart", input: { chart_type: "line_chart" } })
      ])
    );
    renderThread();
    expect(pills("Render Chart").map((p) => p.getAttribute("aria-pressed"))).toEqual([
      "false",
      "false"
    ]);

    selectInTrace("Render Chart", 1);
    expect(pills("Render Chart").map((p) => p.getAttribute("aria-pressed"))).toEqual([
      "false",
      "true"
    ]);

    selectInTrace("Render Chart", 1);
    expect(pills("Render Chart")[1]).toHaveAttribute("aria-pressed", "false");
  });

  it("presses a builder delegation's pill while its panel is open", () => {
    mockUseAnalyticsRun.mockReturnValue(
      runningWith([
        sseEv("step_start", { label: "Solving" }),
        sseEv("delegation_started", {
          child_task_id: "child-1",
          target: "agent:__builder__",
          request: "add a view"
        })
      ])
    );
    renderThread();
    expect(pills("Building")[0]).toHaveAttribute("aria-pressed", "false");
    selectInTrace("Building");
    expect(pills("Building")[0]).toHaveAttribute("aria-pressed", "true");
  });

  it("presses a builder delegation card's View details while its panel is open", () => {
    mockUseAnalyticsRun.mockReturnValue(
      runningWith([
        sseEv("step_start", { label: "Solving" }),
        sseEv("delegation_started", {
          child_task_id: "child-1",
          target: "agent:__builder__",
          request: "add a view"
        })
      ])
    );
    renderThread();
    const viewDetails = () =>
      within(screen.getByTestId("builder-delegation-card")).getByTestId("view-details-button");
    expect(viewDetails()).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(viewDetails());
    expect(screen.getByTestId("delegation-panel")).toBeInTheDocument();
    expect(viewDetails()).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(viewDetails());
    expect(screen.queryByTestId("delegation-panel")).toBeNull();
    expect(viewDetails()).toHaveAttribute("aria-pressed", "false");
  });

  it("presses a running automation card's View details while its run is open", () => {
    mockUseAnalyticsRun.mockReturnValue(
      runningWith(automationStarted("my_proc", [{ name: "step_a", task_type: "execute_sql" }]))
    );
    renderThread();
    const viewDetails = () =>
      within(screen.getByTestId("automation-delegation-card")).getByTestId("view-details-button");
    expect(viewDetails()).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(viewDetails());
    expect(screen.getByRole("heading", { name: "my_proc" })).toBeInTheDocument();
    expect(viewDetails()).toHaveAttribute("aria-pressed", "true");
    // The pill for the same automation says the same.
    expect(pills("my_proc")[0]).toHaveAttribute("aria-pressed", "true");
  });

  it("presses a file-change pill while its file preview is open", () => {
    mockUseQuery.mockReturnValue({
      data: [pastBuilderRunWriting("models/orders.view.yml")],
      isLoading: false,
      isFetching: false
    });
    renderThread(BUILDER_THREAD);
    const filePill = screen.getByRole("button", { name: "orders" });
    expect(filePill).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(filePill);
    expect(filePill).toHaveAttribute("aria-pressed", "true");

    fireEvent.click(filePill);
    expect(filePill).toHaveAttribute("aria-pressed", "false");
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

  it("shows a past run's chart, not the live run's, while another run streams", () => {
    const past = chartRendered(BAR, [["Jan", 100]]);
    const pastEvents = [sseEv("step_start", { label: "Interpreting" }), ...past.events];
    // Event seqs restart in every run, so both render_chart calls carry the same seq.
    counter = 0;
    const live = chartRendered(LINE, [["2024-01", 500]]);
    mockUseQuery.mockReturnValue({
      data: [pastRun({ ui_events: pastEvents.map(sseEventToUiBlock) })],
      isLoading: false,
      isFetching: false
    });
    mockUseAnalyticsRun.mockReturnValue(
      runningWith([sseEv("step_start", { label: "Interpreting" }), ...live.events])
    );
    renderThread();

    selectInTrace("Render Chart", 0);
    expect(panel("Render Chart").getByTestId("display-block").textContent).toBe("Revenue");
  });
});
