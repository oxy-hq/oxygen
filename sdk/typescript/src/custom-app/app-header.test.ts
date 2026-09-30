import { describe, expect, it, vi } from "vitest";
import { APP_HEADER, type AppFetcher, withAppHeader } from "./react";

function recorder() {
  const calls: Array<{ input: unknown; init?: RequestInit }> = [];
  const base = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
    calls.push({ input, init });
    return new Response(null, { status: 204 });
  }) as unknown as AppFetcher;
  return { base, calls };
}

describe("withAppHeader", () => {
  it("names the app on same-origin data-plane calls", async () => {
    const { base, calls } = recorder();
    const f = withAppHeader(base, undefined, "app-1");
    await f("/api/projects/p1/semantic-query", {
      method: "POST",
      headers: { "content-type": "application/json" }
    });
    const headers = new Headers(calls[0].init?.headers);
    expect(headers.get(APP_HEADER)).toBe("app-1");
    expect(headers.get("content-type")).toBe("application/json");
  });

  it("leaves other paths, cross-origin backends and unknown apps alone", async () => {
    const { base, calls } = recorder();
    await withAppHeader(base, undefined, "app-1")("/api/logout");
    await withAppHeader(base, "http://localhost:3000", "app-1")("/api/projects/p1/query");
    await withAppHeader(base, undefined, undefined)("/api/projects/p1/query");
    for (const c of calls) {
      expect(new Headers(c.init?.headers).has(APP_HEADER)).toBe(false);
    }
  });
});
