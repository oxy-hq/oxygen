import { describe, expect, it } from "vitest";
import type { AppEnvironment } from "@/services/api/customApps";
import { chosenPlanes, deletion, fetchHosts, resettablePlanes, untilText } from "./utils";

const env = (over: Partial<AppEnvironment> = {}): AppEnvironment => ({
  name: "dev-a1",
  kind: "dev",
  status: "active",
  build_id: null,
  build_uuid: null,
  semantic_revision_id: null,
  owner: null,
  created_at: "2026-11-01T00:00:00Z",
  updated_at: "2026-11-01T00:00:00Z",
  last_activity_at: null,
  expires_at: null,
  url: null,
  oltp_schema: null,
  parent: "staging",
  fork_at: "2026-11-01T00:00:00Z",
  planes: { oltp: "schema", airhouse: "write-only", messages: "invoker" },
  planes_error: null,
  planes_set: {},
  oltp_branch: null,
  ...over
});

describe("deletion", () => {
  it("is the idle expiry when nothing copies production", () => {
    const d = deletion(env({ expires_at: "2026-11-08T00:00:00Z" }));
    expect(d).toEqual({ at: new Date("2026-11-08T00:00:00Z"), reason: "idle" });
  });

  it("is 30 days past the fork when Airhouse holds a copy, if that comes first", () => {
    const d = deletion(
      env({
        expires_at: "2026-12-31T00:00:00Z",
        planes: { airhouse: "snapshot" }
      })
    );
    expect(d).toEqual({ at: new Date("2026-12-01T00:00:00Z"), reason: "30 days of copied data" });
  });

  it("takes the OLTP branch's own expiry", () => {
    const d = deletion(
      env({
        expires_at: "2026-12-31T00:00:00Z",
        oltp_branch: {
          mode: "branch",
          status: "ready",
          fork_at: "2026-11-01T00:00:00Z",
          expires_at: "2026-11-20T00:00:00Z",
          error: null
        }
      })
    );
    expect(d?.at).toEqual(new Date("2026-11-20T00:00:00Z"));
  });

  it("is null for staging and production, which are never deleted", () => {
    expect(deletion(env({ kind: "staging", expires_at: "2026-11-08T00:00:00Z" }))).toBeNull();
    expect(deletion(env({ kind: "production" }))).toBeNull();
  });
});

describe("resettablePlanes", () => {
  it("offers a sandbox's own copies, and OLTP only on its own branch", () => {
    expect(resettablePlanes(env())).toEqual(["storage", "secrets", "messages"]);
    expect(resettablePlanes(env({ planes: { oltp: "branch", airhouse: "branch" } }))).toEqual([
      "oltp",
      "airhouse",
      "storage",
      "secrets",
      "messages"
    ]);
  });

  it("offers staging only its outbox, and only when it captures", () => {
    expect(resettablePlanes(env({ kind: "staging", planes: { messages: "invoker" } }))).toEqual([]);
    expect(resettablePlanes(env({ kind: "staging", planes: { messages: "capture" } }))).toEqual([
      "messages"
    ]);
  });
});

describe("chosenPlanes and fetchHosts", () => {
  it("lists only the planes the row set, in plane order, with object choices' mode", () => {
    const e = env({
      planes_set: {
        messages: "capture",
        fetch: { mode: "allow", hosts: ["connect.squareupsandbox.com"] },
        oltp: "branch"
      }
    });
    expect(chosenPlanes(e)).toEqual([
      { plane: "oltp", mode: "branch" },
      { plane: "messages", mode: "capture" },
      { plane: "fetch", mode: "allow" }
    ]);
    expect(fetchHosts(e)).toEqual(["connect.squareupsandbox.com"]);
  });
});

describe("untilText", () => {
  const now = new Date("2026-11-03T12:00:00Z");
  it("counts whole days ahead", () => {
    expect(untilText(new Date("2026-11-03T18:00:00Z"), now)).toBe("tomorrow");
    expect(untilText(new Date("2026-11-15T12:00:00Z"), now)).toBe("in 12 days");
    expect(untilText(new Date("2026-11-01T00:00:00Z"), now)).toBe("today");
  });
});
