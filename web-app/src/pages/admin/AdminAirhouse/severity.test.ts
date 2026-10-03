import { describe, expect, it } from "vitest";
import type { AirhouseFleetRow } from "@/services/api/airhouseAdmin";
import { bySeverityThenName, countBySeverity, severityOf } from "./severity";

function row(overrides: Partial<AirhouseFleetRow> = {}): AirhouseFleetRow {
  return {
    workspace_id: "ws",
    workspace_name: "Workspace",
    org_id: "org",
    org_name: "Org",
    status: "active",
    tenant_id: "tenant",
    bucket: "bucket",
    prefix: "",
    service_account_ready: true,
    sa_rotated_at: null,
    created_at: "2026-01-15T10:00:00Z",
    service_account_id: "sa-1",
    sa_created_at: "2026-01-15T10:00:00Z",
    bearer_max_role: "admin",
    bearer_max_ttl_secs: 86_400,
    ...overrides
  };
}

describe("severityOf", () => {
  it("ranks the silent failure above the loud one", () => {
    // The counterintuitive case, and the reason this is a module rather than an
    // inline ternary. A tenant that says `active` with no usable service account
    // reads as fine on every other surface and fails on the first query — the
    // operator has no other signal. A `failed` tenant is already declaring
    // itself, so it is the lesser emergency even though it sounds worse.
    expect(severityOf(row({ status: "active", service_account_ready: false }))).toBe("broken");
    expect(severityOf(row({ status: "failed" }))).toBe("degraded");
  });

  it("does not promote an already-failed tenant just because it also lacks an account", () => {
    // `broken` means "looks healthy, isn't". A row that already says `failed`
    // has nothing hidden about it, so it stays `degraded` and doesn't crowd the
    // top of the queue above tenants whose breakage is invisible.
    expect(severityOf(row({ status: "failed", service_account_ready: false }))).toBe("degraded");
  });

  it("is healthy only when active with a usable service account", () => {
    expect(severityOf(row())).toBe("healthy");
  });
});

describe("bySeverityThenName", () => {
  it("puts every broken tenant above every degraded one, and both above healthy", () => {
    const rows = [
      row({ workspace_name: "aaa-healthy" }),
      row({ workspace_name: "bbb-degraded", status: "failed" }),
      row({ workspace_name: "ccc-broken", service_account_ready: false })
    ];
    expect([...rows].sort(bySeverityThenName).map((r) => r.workspace_name)).toEqual([
      "ccc-broken",
      "bbb-degraded",
      "aaa-healthy"
    ]);
  });

  it("breaks ties by name so equal rows hold a stable order", () => {
    // Without the tiebreak the list reshuffles on every refetch, and an operator
    // scanning it loses their place for no reason.
    const rows = [
      row({ workspace_name: "Zulu" }),
      row({ workspace_name: "Alpha" }),
      row({ workspace_name: "Mike" })
    ];
    expect([...rows].sort(bySeverityThenName).map((r) => r.workspace_name)).toEqual([
      "Alpha",
      "Mike",
      "Zulu"
    ]);
  });

  it("is a total order — sorting a shuffled list twice gives the same answer", () => {
    const rows = [
      row({ workspace_name: "b", service_account_ready: false }),
      row({ workspace_name: "a", status: "pending" }),
      row({ workspace_name: "c" }),
      row({ workspace_name: "a", service_account_ready: false })
    ];
    const once = [...rows].sort(bySeverityThenName);
    const twice = [...rows].reverse().sort(bySeverityThenName);
    expect(twice.map((r) => `${severityOf(r)}:${r.workspace_name}`)).toEqual(
      once.map((r) => `${severityOf(r)}:${r.workspace_name}`)
    );
  });
});

describe("countBySeverity", () => {
  it("partitions the fleet — every row lands in exactly one bucket", () => {
    // The chips are a filter, so the counts have to add up to the list they
    // filter. A row in two buckets (or none) makes "All 12" disagree with the
    // chips beside it, and an operator has no way to tell which lied.
    const rows = [
      row({ service_account_ready: false }),
      row({ status: "failed" }),
      row({ status: "pending" }),
      row(),
      row()
    ];
    const counts = countBySeverity(rows);
    expect(counts).toEqual({ broken: 1, degraded: 2, healthy: 2 });
    expect(counts.broken + counts.degraded + counts.healthy).toBe(rows.length);
  });

  it("reports zeroes rather than omitting a severity", () => {
    // The chip row renders every severity, disabled at zero. An absent key would
    // render `undefined` in the label.
    expect(countBySeverity([])).toEqual({ broken: 0, degraded: 0, healthy: 0 });
  });
});
