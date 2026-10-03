// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { UseAnalyticsRunResult } from "@/hooks/useAnalyticsRun";
import { encodeBase64 } from "@/libs/encoding";
// Imported statically, not via `await import()` inside each test. This module
// graph is heavy (lottie, chart displays) and takes seconds to load cold; paid
// inside a test it counted against the 5s test timeout, and the full parallel
// suite blew through it on a loaded machine. The abort then left that test's
// DOM mounted, so the next test found two textboxes. A static import is
// resolved during the file's load phase, which no test or hook timeout bounds.
// `vi.mock` calls below are hoisted above this by vitest, so the mocks still
// apply.
import AgenticAnalyticsPreview, { getAgentDisplayName, getAgentIdFromPath } from "./index";

// ── Module mocks ──────────────────────────────────────────────────────────────

// Prevent lottie-web from crashing jsdom (no canvas support)
vi.mock("@lottiefiles/react-lottie-player", () => ({ Player: "div" }));

// Avoid pulling in chart/canvas rendering in these unit tests
vi.mock("@/components/AppPreview/Displays", () => ({
  DisplayBlock: () => <div data-testid='display-block' />
}));

const { mockUseAnalyticsRun } = vi.hoisted(() => ({
  mockUseAnalyticsRun: vi.fn<() => UseAnalyticsRunResult>()
}));

vi.mock("@/hooks/useAnalyticsRun", async (importOriginal) => {
  const original = await importOriginal<typeof import("@/hooks/useAnalyticsRun")>();
  return { ...original, useAnalyticsRun: mockUseAnalyticsRun };
});

vi.mock("@/hooks/useCurrentProjectBranch", () => ({
  default: () => ({ project: { id: "proj-1" }, branchName: "main" })
}));

vi.mock("@/pages/thread/analytics/AnalyticsReasoningTrace", () => ({
  default: ({ isRunning }: { isRunning: boolean }) => (
    <div data-testid='reasoning-trace' data-running={String(isRunning)} />
  )
}));

vi.mock("@/pages/thread/analytics/SuspensionPrompt", () => ({
  default: ({ questions }: { questions: { prompt: string }[] }) => (
    <div data-testid='suspension-prompt'>{questions.map((q) => q.prompt).join(",")}</div>
  )
}));

// ── Helpers ───────────────────────────────────────────────────────────────────

const idleResult = (): UseAnalyticsRunResult => ({
  state: { tag: "idle" },
  start: vi.fn(),
  reconnect: vi.fn(),
  hydrate: vi.fn(),
  answer: vi.fn(),
  stop: vi.fn(),
  reset: vi.fn(),
  isStarting: false,
  isAnswering: false
});

const pathb64 = encodeBase64("demo_project/analytics.agentic.yml");

afterEach(() => {
  cleanup();
});

// ── Tests ─────────────────────────────────────────────────────────────────────

describe("AgenticAnalyticsPreview", () => {
  it("shows empty state when no run has started", async () => {
    mockUseAnalyticsRun.mockReturnValue(idleResult());
    render(<AgenticAnalyticsPreview pathb64={pathb64} />);
    expect(screen.getByText("No messages yet")).toBeTruthy();
  });

  it("renders a textarea input and submit button", async () => {
    mockUseAnalyticsRun.mockReturnValue(idleResult());
    render(<AgenticAnalyticsPreview pathb64={pathb64} />);
    expect(screen.getByRole("textbox")).toBeTruthy();
    expect(screen.getByRole("button")).toBeTruthy();
  });

  it("calls start with derived agentId when the form is submitted", async () => {
    const start = vi.fn();
    mockUseAnalyticsRun.mockReturnValue({ ...idleResult(), start });
    render(<AgenticAnalyticsPreview pathb64={pathb64} />);

    const textarea = screen.getByRole("textbox");
    fireEvent.change(textarea, { target: { value: "How many users?" } });
    const form = textarea.closest("form");
    if (!form) throw new Error("form not found");
    fireEvent.submit(form);

    expect(start).toHaveBeenCalledWith(
      "demo_project/analytics.agentic.yml", // full relative path — agent_id for backend
      "How many users?", // question
      expect.any(String) // threadId
    );
  });

  it("disables textarea while state is running", async () => {
    mockUseAnalyticsRun.mockReturnValue({
      ...idleResult(),
      state: { tag: "running", runId: "r1", events: [] }
    });
    render(<AgenticAnalyticsPreview pathb64={pathb64} />);
    expect((screen.getByRole("textbox") as HTMLTextAreaElement).disabled).toBe(true);
  });

  it("disables textarea while isStarting", async () => {
    mockUseAnalyticsRun.mockReturnValue({ ...idleResult(), isStarting: true });
    render(<AgenticAnalyticsPreview pathb64={pathb64} />);
    expect((screen.getByRole("textbox") as HTMLTextAreaElement).disabled).toBe(true);
  });

  it("renders AnalyticsReasoningTrace when state is running with events", async () => {
    mockUseAnalyticsRun.mockReturnValue({
      ...idleResult(),
      state: {
        tag: "running",
        runId: "r1",
        events: [{ id: "1", type: "step_start", data: { label: "Querying" } }]
      }
    });
    render(<AgenticAnalyticsPreview pathb64={pathb64} />);
    expect(screen.getByTestId("reasoning-trace")).toBeTruthy();
    expect(screen.getByTestId("reasoning-trace").dataset.running).toBe("true");
  });

  it("renders SuspensionPrompt when state is suspended", async () => {
    mockUseAnalyticsRun.mockReturnValue({
      ...idleResult(),
      state: {
        tag: "suspended",
        runId: "r1",
        events: [],
        questions: [{ prompt: "What date range?", suggestions: [] }]
      }
    });
    render(<AgenticAnalyticsPreview pathb64={pathb64} />);
    expect(screen.getByTestId("suspension-prompt")).toBeTruthy();
    expect(screen.getByTestId("suspension-prompt").textContent).toContain("What date range?");
  });
});

describe("getAgentIdFromPath", () => {
  it("returns the full path as agent_id (backend resolves relative to project root)", async () => {
    expect(getAgentIdFromPath("demo_project/analytics.agentic.yml")).toBe(
      "demo_project/analytics.agentic.yml"
    );
  });

  it("passes through flat paths unchanged", async () => {
    expect(getAgentIdFromPath("analytics.agentic.yml")).toBe("analytics.agentic.yml");
  });
});

describe("getAgentDisplayName", () => {
  it("extracts display name from .agentic.yml path", async () => {
    expect(getAgentDisplayName("demo_project/analytics.agentic.yml")).toBe("analytics");
  });

  it("extracts display name from .agentic.yaml path", async () => {
    expect(getAgentDisplayName("training_coach.agentic.yaml")).toBe("training_coach");
  });

  it("handles underscore names", async () => {
    expect(getAgentDisplayName("app_builder.agentic.yml")).toBe("app_builder");
  });
});
