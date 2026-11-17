import { describe, expect, it } from "vitest";
import type { FunctionInvocation, InvocationCalls } from "@/types/apps";
import { invocationBrief } from "./invocationBrief";

const app = { id: "app-1", org_slug: "poke-house", slug: "bookkeeping" };

const iv = (over: Partial<FunctionInvocation> = {}): FunctionInvocation => ({
  id: "inv-9",
  mode: "route",
  status: "error",
  failed: true,
  result_status: null,
  duration_ms: 812,
  error: "TypeError: cannot read properties of undefined (reading 'rows')",
  created_at: "2026-11-17T05:00:00Z",
  has_result: false,
  ...over
});

const calls = (over: Partial<InvocationCalls> = {}): InvocationCalls => ({
  invocation_id: "inv-9",
  function: "ingest",
  environment: "staging",
  build_id: "b-7",
  mode: "route",
  status: "error",
  failed: true,
  duration_ms: 812,
  created_at: "2026-11-17T05:00:00Z",
  recorded: true,
  reason: null,
  init_ms: 40,
  total_calls: 3,
  dropped: 0,
  unfinished: 0,
  retention_days: 14,
  calls: [
    { seq: 1, op: "query", family: "query", start_ms: 41, duration_ms: 90, status: "ok" },
    {
      seq: 2,
      op: "oltp.exec",
      family: "oltp",
      start_ms: 140,
      duration_ms: 10,
      status: "refused",
      error_type: "not_allowed",
      verb: "INSERT",
      target: "app.statements"
    },
    {
      seq: 3,
      op: "fetch",
      family: "fetch",
      start_ms: 200,
      duration_ms: 600,
      status: "error",
      error_type: "upstream_5xx",
      target: "api.vendor.example",
      http_status: 502
    }
  ],
  ...over
});

describe("invocationBrief", () => {
  it("says what ran, where, how it ended, and which ctx calls failed", () => {
    const text = invocationBrief(app, "ingest", iv(), calls());
    expect(text).toContain("poke-house/bookkeeping");
    expect(text).toContain("Function:     ingest");
    expect(text).toContain("Environment:  staging");
    expect(text).toContain("Build:        b-7");
    expect(text).toContain("error after 812ms");
    expect(text).toContain("TypeError: cannot read properties");
    expect(text).toContain("3 made, 2 failed or refused");
    expect(text).toContain("#2 oltp.exec INSERT app.statements: refused (not_allowed)");
    expect(text).toContain("#3 fetch api.vendor.example: error (upstream_5xx), HTTP 502");
    expect(text).not.toContain("#1 query");
  });

  it("gives the reads that fetch the rest, by this invocation's id", () => {
    const text = invocationBrief(app, "ingest", iv(), calls());
    expect(text).toContain("/logs?hours=168&invocation_id=inv-9");
    expect(text).toContain("/api/customer-apps/app-1/invocations/inv-9/calls");
  });

  it("says the trace was not fetched rather than that it was empty", () => {
    const text = invocationBrief(app, "ingest", iv());
    expect(text).toContain("ctx calls: not fetched");
    expect(text).toContain("Environment:  unknown");
  });

  it("says why there is no trace when the server kept none", () => {
    const text = invocationBrief(
      app,
      "ingest",
      iv(),
      calls({ recorded: false, reason: "expired", calls: [] })
    );
    expect(text).toContain("ctx calls: no trace (expired)");
  });

  it("names a timeout with no error text instead of leaving a blank line", () => {
    const text = invocationBrief(app, "ingest", iv({ status: "timeout", error: null }));
    expect(text).toContain("Timed out. No error text is recorded");
  });
});
