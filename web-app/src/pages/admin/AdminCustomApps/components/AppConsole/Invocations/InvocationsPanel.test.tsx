// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AppInvocation, AppInvocationList, AppInvocationQuery, CustomApp } from "@/types/apps";
import { InvocationsPanel } from ".";

const asked: AppInvocationQuery[] = [];
let current: AppInvocationList = { invocations: [] };
let previous: AppInvocationList = { invocations: [] };

// The hooks are the seam: this file is about what the panel claims from what the
// server answered, not React Query's plumbing.
vi.mock("@/hooks/api/customApps/useAppInvocations", () => ({
  useAppInvocations: (_app: string, query: AppInvocationQuery) => {
    asked.push(query);
    const data = query.limit === 1 ? previous : current;
    return { isPending: false, isError: false, isFetching: false, data, refetch: vi.fn() };
  }
}));
vi.mock("@/hooks/api/customApps/useAppFunctions", () => ({
  useAppFunctions: () => ({ data: [{ name: "sync" }, { name: "nightly" }] }),
  useInvocationCalls: () => ({ data: undefined })
}));

const APP = { id: "app-1", org_slug: "poke-house", slug: "bookkeeping" } as CustomApp;

const rows = (n: number, failed = 0): AppInvocation[] =>
  Array.from({ length: n }, (_, i) => ({
    id: `iv-${i}`,
    function_name: i % 2 ? "sync" : "nightly",
    mode: i % 2 ? "route" : "schedule",
    environment: "production",
    build_id: "b-1",
    build_uuid: "u-1",
    status: i < failed ? "error" : "success",
    failed: i < failed,
    result_status: null,
    duration_ms: 50 + i,
    error: i < failed ? "boom" : null,
    error_kind: null,
    created_at: "2026-11-17T05:00:00Z",
    has_result: false
  }));

const panel = () =>
  render(<InvocationsPanel app={APP} environment='production' scope={{ label: "Production" }} />);

afterEach(() => {
  cleanup();
  asked.length = 0;
});

describe("InvocationsPanel", () => {
  it("names production on every read, since no environment means every one to staff", () => {
    current = { invocations: rows(3), window: { total: 3, failed: 0 } };
    panel();
    expect(asked.length).toBeGreaterThan(0);
    for (const q of asked) expect(q.environment).toBe("production");
    expect(asked.some((q) => q.since && q.until && q.limit === 200)).toBe(true);
  });

  it("says the list is the newest 200 when the window holds more, and counts the window", () => {
    current = { invocations: rows(200, 3), window: { total: 1412, failed: 37 } };
    previous = { invocations: rows(1), window: { total: 1260, failed: 12 } };
    panel();
    expect(screen.getByTestId("admin-app-invocations-capped").textContent).toContain(
      "newest 200 of 1,412"
    );
    expect(screen.getByTestId("admin-app-invocations-latency-of").textContent).toContain(
      "of the newest 200 of 1,412"
    );
    expect(screen.getByTestId("admin-app-invocations-total").textContent).toBe("1,412");
    const failed = screen.getByTestId("admin-app-invocations-failed");
    expect(failed.textContent).toBe("37");
    expect(failed.className).toContain(ADMIN_TONE.danger.text);
    expect(screen.getByTestId("admin-app-invocations-summary").textContent).toContain(
      "+12% on 1,260 the 24h before"
    );
  });

  it("claims the whole window only when the page holds all of it, and colours no zero", () => {
    current = { invocations: rows(12), window: { total: 12, failed: 0 } };
    previous = { invocations: [], window: { total: 0, failed: 0 } };
    panel();
    expect(screen.queryByTestId("admin-app-invocations-capped")).toBeNull();
    expect(screen.getByTestId("admin-app-invocations-latency-of").textContent).toContain(
      "of all 12 in the window"
    );
    expect(screen.getByTestId("admin-app-invocations-failed").className).not.toContain(
      ADMIN_TONE.danger.text
    );
  });
});
