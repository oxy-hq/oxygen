// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, renderHook, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppBuildChecksService } from "@/services/api/appBuildChecks";
import type { BuildChecks } from "@/types/appBuildChecks";
import {
  BUILD_CHECKS_NOTES_WAIT_MS,
  BUILD_CHECKS_POLL_MS,
  buildChecksPollMs,
  useBuildChecks,
  useMarkChangesSeen
} from "./useBuildChecks";

vi.mock("@/services/api/appBuildChecks", () => ({
  AppBuildChecksService: { get: vi.fn(), markChangesSeen: vi.fn() }
}));

const answer = (state: BuildChecks["state"]) => ({ state, checks: [] }) as unknown as BuildChecks;

const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
    {children}
  </QueryClientProvider>
);

afterEach(() => {
  vi.useRealTimers();
  vi.mocked(AppBuildChecksService.get).mockReset();
  vi.mocked(AppBuildChecksService.markChangesSeen).mockReset();
});

describe("useBuildChecks", () => {
  it("re-reads while the checks run, and stops once they settle", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const get = vi.mocked(AppBuildChecksService.get);
    get.mockResolvedValueOnce(answer("running")).mockResolvedValue(answer("passed"));

    const { result } = renderHook(() => useBuildChecks("app-1", "row-s"), { wrapper });
    await waitFor(() => expect(result.current.data?.state).toBe("running"));
    expect(get).toHaveBeenCalledWith("app-1", "row-s");

    await act(() => vi.advanceTimersByTimeAsync(BUILD_CHECKS_POLL_MS));
    await waitFor(() => expect(result.current.data?.state).toBe("passed"));
    const settled = get.mock.calls.length;
    expect(settled).toBe(2);

    await act(() => vi.advanceTimersByTimeAsync(BUILD_CHECKS_POLL_MS * 3));
    expect(get).toHaveBeenCalledTimes(settled);
  });

  it("stops once an attempt stalls: it arrives as failed, with why", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const get = vi.mocked(AppBuildChecksService.get);
    const stalled = { ...answer("failed"), runner: null, stalled: "no_runner" } as BuildChecks;
    get.mockResolvedValueOnce(answer("running")).mockResolvedValue(stalled);

    const { result } = renderHook(() => useBuildChecks("app-1", "row-s"), { wrapper });
    await waitFor(() => expect(result.current.data?.state).toBe("running"));
    await act(() => vi.advanceTimersByTimeAsync(BUILD_CHECKS_POLL_MS));
    await waitFor(() => expect(result.current.data?.stalled).toBe("no_runner"));
    expect(get).toHaveBeenCalledTimes(2);

    await act(() => vi.advanceTimersByTimeAsync(BUILD_CHECKS_POLL_MS * 3));
    expect(get).toHaveBeenCalledTimes(2);
  });

  it("asks nothing without a build", () => {
    renderHook(() => useBuildChecks("app-1", undefined), { wrapper });
    expect(AppBuildChecksService.get).not.toHaveBeenCalled();
  });
});

const NOW = Date.parse("2026-11-25T09:00:00Z");
const ago = (ms: number) => new Date(NOW - ms).toISOString();

/** A staging attempt whose roll-up has settled, with the gate's `changes` row. */
const settled = (over: Partial<BuildChecks> = {}, changes = "passed") =>
  ({
    state: "passed",
    environment: "staging",
    summary: null,
    checks: [
      { kind: "smoke", status: "passed", finished_at: ago(20_000) },
      { kind: "changes", status: changes, finished_at: changes === "running" ? null : ago(10_000) }
    ],
    ...over
  }) as unknown as BuildChecks;

describe("buildChecksPollMs — when a build's checks are read again", () => {
  it("reads on while the roll-up runs, and stops with nothing to read", () => {
    expect(buildChecksPollMs(answer("running"), NOW)).toBe(BUILD_CHECKS_POLL_MS);
    expect(buildChecksPollMs(answer("passed"), NOW)).toBe(false);
    expect(buildChecksPollMs(undefined, NOW)).toBe(false);
  });

  // The roll-up settles on the checks that decide it; "what changed" only informs and
  // can finish seconds later. Stopping here left its row reading `running` for good.
  it("reads on while a check that only informs is still open", () => {
    expect(buildChecksPollMs(settled({ state: "failed" }, "running"), NOW)).toBe(
      BUILD_CHECKS_POLL_MS
    );
    expect(buildChecksPollMs(settled({}, "queued"), NOW)).toBe(BUILD_CHECKS_POLL_MS);
  });

  it("waits a bounded while for the model's notes, which follow the last verdict", () => {
    expect(buildChecksPollMs(settled(), NOW)).toBe(BUILD_CHECKS_POLL_MS);
    expect(buildChecksPollMs(settled(), NOW + BUILD_CHECKS_NOTES_WAIT_MS)).toBe(false);
  });

  it("stops once the summary is in: it is written last", () => {
    const summary = { text: "No checks failed.", model: "m", created_at: ago(1_000) };
    expect(buildChecksPollMs(settled({ summary } as Partial<BuildChecks>), NOW)).toBe(false);
  });

  it("expects no notes on production, nor where no screens were compared", () => {
    expect(buildChecksPollMs(settled({ environment: "production" }), NOW)).toBe(false);
    const phaseOne = settled();
    phaseOne.checks = phaseOne.checks.filter((c) => c.kind !== "changes");
    expect(buildChecksPollMs(phaseOne, NOW)).toBe(false);
  });
});

describe("useBuildChecks — after the roll-up settles", () => {
  it("still re-reads a build whose changes check is running", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const get = vi.mocked(AppBuildChecksService.get);
    const done = { text: "Two screens differ.", model: "m", created_at: ago(0) };
    get
      .mockResolvedValueOnce(settled({ state: "failed" }, "running"))
      .mockResolvedValue(settled({ state: "failed", summary: done } as Partial<BuildChecks>));

    const { result } = renderHook(() => useBuildChecks("app-1", "row-s"), { wrapper });
    await waitFor(() => expect(result.current.data?.checks[1].status).toBe("running"));

    await act(() => vi.advanceTimersByTimeAsync(BUILD_CHECKS_POLL_MS));
    await waitFor(() => expect(result.current.data?.checks[1].status).toBe("passed"));
    expect(get).toHaveBeenCalledTimes(2);

    await act(() => vi.advanceTimersByTimeAsync(BUILD_CHECKS_POLL_MS * 3));
    expect(get).toHaveBeenCalledTimes(2);
  });
});

describe("useBuildChecks — the attempt run on production", () => {
  it("asks for it by name, and re-reads it while it runs, by the same rule", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const get = vi.mocked(AppBuildChecksService.get);
    get.mockResolvedValueOnce(answer("running")).mockResolvedValue(answer("failed"));

    const { result } = renderHook(() => useBuildChecks("app-1", "row-p", "production"), {
      wrapper
    });
    await waitFor(() => expect(result.current.data?.state).toBe("running"));
    expect(get).toHaveBeenCalledWith("app-1", "row-p", "production");

    await act(() => vi.advanceTimersByTimeAsync(BUILD_CHECKS_POLL_MS));
    await waitFor(() => expect(result.current.data?.state).toBe("failed"));
    expect(get).toHaveBeenCalledTimes(2);

    await act(() => vi.advanceTimersByTimeAsync(BUILD_CHECKS_POLL_MS * 3));
    expect(get).toHaveBeenCalledTimes(2);
  });

  it("keeps the two attempts of one build apart", async () => {
    const get = vi.mocked(AppBuildChecksService.get);
    get.mockImplementation(async (_app, _build, environment) =>
      answer(environment === "production" ? "failed" : "passed")
    );
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const shared = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const staging = renderHook(() => useBuildChecks("app-1", "row-p"), { wrapper: shared });
    const production = renderHook(() => useBuildChecks("app-1", "row-p", "production"), {
      wrapper: shared
    });
    await waitFor(() => expect(staging.result.current.data?.state).toBe("passed"));
    await waitFor(() => expect(production.result.current.data?.state).toBe("failed"));
  });
});

describe("useMarkChangesSeen", () => {
  it("marks one attempt seen, then re-reads the app's checks", async () => {
    const get = vi.mocked(AppBuildChecksService.get);
    get.mockResolvedValue(answer("passed"));
    vi.mocked(AppBuildChecksService.markChangesSeen).mockResolvedValue();
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const shared = ({ children }: { children: ReactNode }) => (
      <QueryClientProvider client={client}>{children}</QueryClientProvider>
    );
    const checks = renderHook(() => useBuildChecks("app-1", "row-s"), { wrapper: shared });
    await waitFor(() => expect(checks.result.current.data?.state).toBe("passed"));
    expect(get).toHaveBeenCalledTimes(1);

    const mark = renderHook(() => useMarkChangesSeen("app-1"), { wrapper: shared });
    await act(() => mark.result.current.mutateAsync({ build: "row-s", attempt: 2 }));
    expect(AppBuildChecksService.markChangesSeen).toHaveBeenCalledWith("app-1", "row-s", 2);
    await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
  });
});
