// @vitest-environment jsdom

import { act, cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppFunctionSummary, CustomApp } from "@/types/apps";
import { SchedulesPanel } from ".";

const refetch = vi.fn();
let functions: AppFunctionSummary[] = [];
let dataUpdatedAt = 0;
let errorUpdatedAt = 0;

// The hooks are the seam: this file is about what the panel shows of a next run
// and when it reads the functions again, not React Query's plumbing.
vi.mock("@/hooks/api/customApps/useAppFunctions", () => ({
  useAppFunctions: () => ({
    isPending: false,
    isError: false,
    data: functions,
    refetch,
    dataUpdatedAt,
    errorUpdatedAt
  }),
  useInvocationCalls: () => ({ data: undefined }),
  useFunctionRun: () => ({ data: undefined })
}));
vi.mock("@/hooks/api/customApps/useAppInvocations", () => ({
  useAppInvocationsMany: (_app: string, queries: unknown[]) =>
    queries.map(() => ({ isPending: false, data: { invocations: [] } })),
  useEnvironmentSchedules: () => ({ isPending: true, isError: false, data: undefined }),
  useRunFunctionAsSchedule: () => ({ mutate: vi.fn(), isPending: false })
}));

const APP = { id: "app-1", org_slug: "poke-house", slug: "bookkeeping" } as CustomApp;
const NOW = Date.parse("2026-11-17T09:18:00Z");

const scheduled = (next_run_at: string): AppFunctionSummary => ({
  name: "sync",
  route: false,
  schedule: "*/15 * * * *",
  timezone: null,
  next_run_at,
  airway: false,
  timeout_seconds: null,
  retries: null,
  secrets_write: false,
  destinations: []
});

const panel = () =>
  render(<SchedulesPanel app={APP} environment='production' scope={{ label: "Production" }} />);

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  refetch.mockReset();
  dataUpdatedAt = 0;
  errorUpdatedAt = 0;
});

describe("SchedulesPanel next run", () => {
  it("reads a next run in the future as next", () => {
    functions = [scheduled("2026-11-17T09:30:00Z")];
    panel();
    const next = screen.getByTestId("admin-app-schedule-next");
    expect(next.getAttribute("data-due")).toBe("false");
    expect(next.textContent).toMatch(/^next /);
    expect(next.textContent).not.toContain("ago");
  });

  it("calls a next run that has passed due, never 'next … ago'", () => {
    functions = [scheduled("2026-11-17T09:15:00Z")];
    panel();
    const next = screen.getByTestId("admin-app-schedule-next");
    expect(next.getAttribute("data-due")).toBe("true");
    expect(next.textContent).toContain("due");
    expect(next.textContent).not.toContain("ago");
    expect(next.textContent).not.toMatch(/^next/);
  });

  it("reads the functions again just after the soonest next run passes", async () => {
    functions = [scheduled("2026-11-17T09:30:00Z")];
    panel();
    await act(() => vi.advanceTimersByTimeAsync(12 * 60_000));
    expect(refetch).not.toHaveBeenCalled();
    await act(() => vi.advanceTimersByTimeAsync(5_000));
    expect(refetch).toHaveBeenCalledTimes(1);
  });

  it("re-reads a passed next run after the floor, not in a loop", async () => {
    functions = [scheduled("2026-11-17T09:15:00Z")];
    panel();
    await act(() => vi.advanceTimersByTimeAsync(29_000));
    expect(refetch).not.toHaveBeenCalled();
    await act(() => vi.advanceTimersByTimeAsync(1_000));
    expect(refetch).toHaveBeenCalledTimes(1);
  });

  // A server whose clock is behind the browser's answers with the same passed
  // instant, so the value never changes; the panel must still read again.
  it("keeps re-reading while the server answers with the same passed instant", async () => {
    functions = [scheduled("2026-11-17T09:15:00Z")];
    const view = panel();
    refetch.mockImplementation(() => {
      dataUpdatedAt += 1;
      view.rerender(
        <SchedulesPanel app={APP} environment='production' scope={{ label: "Production" }} />
      );
    });
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(refetch).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(29_000));
    expect(refetch).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(1_000));
    expect(refetch).toHaveBeenCalledTimes(2);
  });

  it("reads again after a failed read", async () => {
    functions = [scheduled("2026-11-17T09:15:00Z")];
    const view = panel();
    refetch.mockImplementation(() => {
      errorUpdatedAt += 1;
      view.rerender(
        <SchedulesPanel app={APP} environment='production' scope={{ label: "Production" }} />
      );
    });
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(refetch).toHaveBeenCalledTimes(1);
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(refetch).toHaveBeenCalledTimes(2);
  });
});
