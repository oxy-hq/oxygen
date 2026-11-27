// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { BuildChecks } from "@/types/appBuildChecks";
import type { CustomApp } from "@/types/apps";
import queryKeys from "../queryKey";
import {
  FAILED_SMOKE_FRESH_MS,
  FAILED_SMOKE_REREADS_MS,
  failedSmokeAt,
  useFollowFailedSmoke
} from "./useFollowFailedSmoke";

const NOW = Date.parse("2026-11-25T09:14:00Z");
const ago = (ms: number) => new Date(NOW - ms).toISOString();

/** Production's attempt with its smoke `status`, ended at `finished_at`. */
const attempt = (status: string, finished_at: string | null) =>
  ({
    environment: "production",
    checks: [{ kind: "production_smoke", status, finished_at }]
  }) as unknown as BuildChecks;

const appThat = (auto_rollback: boolean) =>
  ({ id: "app-1", promotion_checks: { auto_rollback } }) as Pick<
    CustomApp,
    "id" | "promotion_checks"
  >;

function follow(failedAt: string | null, rollsBack: boolean) {
  const client = new QueryClient();
  const reads = vi.spyOn(client, "invalidateQueries");
  const wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  const production = failedAt ? attempt("failed", failedAt) : attempt("passed", ago(1_000));
  const hook = renderHook(() => useFollowFailedSmoke(appThat(rollsBack), production), { wrapper });
  return { reads, hook };
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(NOW);
});
afterEach(() => vi.useRealTimers());

describe("failedSmokeAt", () => {
  it("is when production's own check failed, and nothing for any other ending", () => {
    expect(failedSmokeAt(attempt("failed", "2026-11-25T09:13:59Z"))).toBe("2026-11-25T09:13:59Z");
    expect(failedSmokeAt(attempt("passed", "2026-11-25T09:13:59Z"))).toBeNull();
    expect(failedSmokeAt(attempt("running", null))).toBeNull();
    expect(failedSmokeAt(undefined)).toBeNull();
  });
});

describe("useFollowFailedSmoke", () => {
  it("re-reads the app's builds a few times after a failure that just arrived", () => {
    const { reads } = follow(ago(1_000), true);
    expect(reads).not.toHaveBeenCalled();

    vi.advanceTimersByTime(FAILED_SMOKE_REREADS_MS[0]);
    expect(reads).toHaveBeenCalledWith({ queryKey: queryKeys.customApps.builds("app-1") });
    expect(reads).toHaveBeenCalledWith({ queryKey: queryKeys.customApps.all() });
    expect(reads).toHaveBeenCalledTimes(2);

    vi.advanceTimersByTime(FAILED_SMOKE_REREADS_MS.at(-1) as number);
    expect(reads).toHaveBeenCalledTimes(2 * FAILED_SMOKE_REREADS_MS.length);

    vi.advanceTimersByTime(FAILED_SMOKE_FRESH_MS * 5);
    expect(reads).toHaveBeenCalledTimes(2 * FAILED_SMOKE_REREADS_MS.length);
  });

  it("reads nothing where the app does not roll back by itself, or nothing failed", () => {
    const noRollback = follow(ago(1_000), false);
    const noFailure = follow(null, true);
    vi.advanceTimersByTime(FAILED_SMOKE_FRESH_MS);
    expect(noRollback.reads).not.toHaveBeenCalled();
    expect(noFailure.reads).not.toHaveBeenCalled();
  });

  it("leaves an old failure alone: the page was opened after it was dealt with", () => {
    const { reads } = follow(ago(FAILED_SMOKE_FRESH_MS), true);
    vi.advanceTimersByTime(FAILED_SMOKE_FRESH_MS);
    expect(reads).not.toHaveBeenCalled();
  });

  it("stops when the card goes away", () => {
    const { reads, hook } = follow(ago(1_000), true);
    hook.unmount();
    vi.advanceTimersByTime(FAILED_SMOKE_FRESH_MS);
    expect(reads).not.toHaveBeenCalled();
  });
});
