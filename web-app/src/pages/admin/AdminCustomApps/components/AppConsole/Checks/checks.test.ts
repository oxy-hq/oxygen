import { describe, expect, it } from "vitest";
import type { AppInvocation } from "@/types/apps";
import {
  type CellRead,
  checkEnvironments,
  checkMatrix,
  checkVerdict,
  failingIn,
  latestPerEnvironment
} from "./checks";

const run = (over: Partial<AppInvocation> = {}): AppInvocation => ({
  id: "inv",
  function_name: "smoke",
  mode: "manual",
  environment: "production",
  build_id: "b-1",
  build_uuid: "00000000-0000-0000-0000-000000000001",
  status: "success",
  failed: false,
  result_status: null,
  duration_ms: 40,
  error: null,
  error_kind: null,
  created_at: "2026-11-17T05:00:00Z",
  has_result: false,
  ...over
});

describe("checkEnvironments", () => {
  it("is production and staging, plus the console's sandbox", () => {
    expect(checkEnvironments("production")).toEqual(["production", "staging"]);
    expect(checkEnvironments("staging")).toEqual(["production", "staging"]);
    expect(checkEnvironments("dev-a1")).toEqual(["production", "staging", "dev-a1"]);
  });
});

describe("latestPerEnvironment", () => {
  it("takes the newest run of each environment, whatever order they arrive in", () => {
    const rows = [
      run({ id: "p-old", created_at: "2026-11-16T05:00:00Z" }),
      run({ id: "s-new", environment: "staging", created_at: "2026-11-17T08:00:00Z" }),
      run({ id: "p-new", created_at: "2026-11-17T06:00:00Z" }),
      run({ id: "s-old", environment: "staging", created_at: "2026-11-15T08:00:00Z" })
    ];
    const latest = latestPerEnvironment(rows, ["production", "staging"]);
    expect(latest.get("production")?.id).toBe("p-new");
    expect(latest.get("staging")?.id).toBe("s-new");
  });

  it("names an environment with no run, and ignores environments it was not asked for", () => {
    const rows = [
      run({ id: "sandbox", environment: "dev-a1", created_at: "2026-11-18T00:00:00Z" })
    ];
    const latest = latestPerEnvironment(rows, ["production", "staging"]);
    expect([...latest.entries()]).toEqual([
      ["production", null],
      ["staging", null]
    ]);
  });
});

describe("checkVerdict", () => {
  it("fails on the platform's rule, not the status word", () => {
    expect(checkVerdict(run({ status: "success", failed: true }))).toBe("failed");
    expect(checkVerdict(run({ status: "error", failed: true }))).toBe("failed");
    expect(checkVerdict(run({ status: "success" }))).toBe("passed");
    expect(checkVerdict(run({ status: "running" }))).toBe("running");
    expect(checkVerdict(run({ status: "cancelled" }))).toBe("other");
    expect(checkVerdict(null)).toBe("never");
  });
});

describe("checkMatrix", () => {
  const reads: Record<string, CellRead> = {
    "smoke|production": {
      rows: [run({ id: "p1", failed: true, status: "error" })],
      pending: false,
      failed: false,
      forbidden: false
    },
    "smoke|staging": { rows: [], pending: false, failed: false, forbidden: false },
    "ledger|production": { pending: true, failed: false, forbidden: false },
    "ledger|staging": { pending: false, failed: true, forbidden: true }
  };
  const matrix = checkMatrix(["smoke", "ledger"], ["production", "staging"], (check, env) => {
    return reads[`${check}|${env}`];
  });

  it("gives every check a cell per environment, saying what it could and could not read", () => {
    expect(matrix.map((r) => r.cells.map((c) => c.state))).toEqual([
      ["read", "read"],
      ["loading", "unreadable"]
    ]);
    const [smokeProd, smokeStaging] = matrix[0].cells;
    expect(smokeProd).toMatchObject({ verdict: "failed", latest: { id: "p1" } });
    expect(smokeStaging).toMatchObject({ verdict: "never", latest: null });
    expect(matrix[1].cells[1]).toMatchObject({ forbidden: true });
  });

  it("lists the checks failing in one environment", () => {
    expect(failingIn(matrix, "production")).toEqual(["smoke"]);
    expect(failingIn(matrix, "staging")).toEqual([]);
  });
});
