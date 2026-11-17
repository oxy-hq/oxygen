// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { AppInvocation, AppInvocationQuery, CustomApp } from "@/types/apps";
import { ChecksPanel } from ".";

const runChecks = vi.fn();
let runs: Record<string, AppInvocation[]> = {};

vi.mock("@/hooks/api/customApps/useAppInvocations", () => ({
  useAppInvocationsMany: (_app: string, queries: AppInvocationQuery[]) =>
    queries.map((q) => ({
      isPending: false,
      isError: false,
      data: { invocations: runs[`${q.function}|${q.environment}`] ?? [] }
    })),
  useRunChecks: () => ({ mutate: runChecks, isPending: false })
}));
vi.mock("@/hooks/api/customApps/useAppFunctions", () => ({
  useAppFunctions: () => ({
    isPending: false,
    isError: false,
    data: [
      { name: "smoke", check: true },
      { name: "sync", check: false },
      { name: "ledger", check: true }
    ]
  }),
  useInvocationCalls: () => ({ data: undefined })
}));

const APP = { id: "app-1", org_slug: "poke-house", slug: "bookkeeping" } as CustomApp;

const run = (over: Partial<AppInvocation>): AppInvocation => ({
  id: "iv",
  function_name: "smoke",
  mode: "manual",
  environment: "production",
  build_id: "b-9",
  build_uuid: "u-9",
  status: "success",
  failed: false,
  result_status: null,
  duration_ms: 30,
  error: null,
  error_kind: null,
  created_at: "2026-11-17T05:00:00Z",
  has_result: false,
  ...over
});

afterEach(() => {
  cleanup();
  runChecks.mockReset();
  runs = {};
});

describe("ChecksPanel", () => {
  it("shows each check's latest run per environment, colouring only the failure", () => {
    runs = {
      "smoke|production": [run({ id: "p" })],
      "smoke|staging": [
        run({ id: "s", environment: "staging", status: "error", failed: true, build_id: "b-10" })
      ]
    };
    render(
      <ChecksPanel app={APP} environment='staging' scope={{ label: "Staging", warn: true }} />
    );
    const smoke = screen.getByTestId("admin-app-check-smoke");
    const [prod, staging] = [
      smoke.querySelector("[data-testid='admin-app-check-cell-production']"),
      smoke.querySelector("[data-testid='admin-app-check-cell-staging']")
    ];
    expect(prod?.getAttribute("data-verdict")).toBe("passed");
    expect(prod?.innerHTML).not.toContain(ADMIN_TONE.danger.text);
    expect(staging?.getAttribute("data-verdict")).toBe("failed");
    expect(staging?.innerHTML).toContain(ADMIN_TONE.danger.text);
    expect(staging?.textContent).toContain("b-10");
    // A check with no run anywhere says so, rather than passing by omission.
    const ledger = screen.getByTestId("admin-app-check-ledger");
    expect(ledger.textContent).toContain("never run");
    // `sync` is not a check.
    expect(screen.queryByTestId("admin-app-check-sync")).toBeNull();
  });

  it("runs every check in the console's environment", () => {
    render(
      <ChecksPanel app={APP} environment='staging' scope={{ label: "Staging", warn: true }} />
    );
    fireEvent.click(screen.getByTestId("admin-app-checks-run"));
    expect(runChecks).toHaveBeenCalledTimes(1);
    expect(runChecks.mock.calls[0][0]).toEqual({
      id: "app-1",
      names: ["smoke", "ledger"],
      environment: "staging"
    });
  });
});
