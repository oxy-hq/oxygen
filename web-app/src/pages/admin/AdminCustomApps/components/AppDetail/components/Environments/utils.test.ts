import { describe, expect, it } from "vitest";
import type { AppEnvironment } from "@/services/api/customApps";
import {
  chosenPlanes,
  deletion,
  fetchHosts,
  oltpRecutQueued,
  resetCopy,
  resettablePlanes,
  settling,
  untilText
} from "./utils";

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

  it("is the server's copied-data expiry, if that comes first", () => {
    const d = deletion(
      env({ expires_at: "2026-12-31T00:00:00Z", copies_expire_at: "2026-12-01T00:00:00Z" })
    );
    expect(d).toEqual({ at: new Date("2026-12-01T00:00:00Z"), reason: "30 days of copied data" });
  });

  it("does not guess a copied-data expiry from the planes: the server states it", () => {
    // A write-only sandbox can still hold copies, and a snapshot one may hold none
    // yet: only the server's sweep rule knows, so the console reads its answer.
    const d = deletion(
      env({ expires_at: "2026-12-31T00:00:00Z", planes: { airhouse: "snapshot" } })
    );
    expect(d?.reason).toBe("idle");
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
          error: null,
          recut_queued: false
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
  it("offers a sandbox's own copies, OLTP only on its own branch, Airhouse on every mode", () => {
    expect(resettablePlanes(env())).toEqual(["airhouse", "storage", "secrets", "messages"]);
    expect(resettablePlanes(env({ planes: { oltp: "branch", airhouse: "branch" } }))).toEqual([
      "oltp",
      "airhouse",
      "storage",
      "secrets",
      "messages"
    ]);
  });

  it("offers nothing on a sandbox being deleted, which refuses every reset", () => {
    expect(resettablePlanes(env({ status: "deleting", planes: { oltp: "branch" } }))).toEqual([]);
  });

  it("offers staging only its outbox, and only when it captures", () => {
    expect(resettablePlanes(env({ kind: "staging", planes: { messages: "invoker" } }))).toEqual([]);
    expect(resettablePlanes(env({ kind: "staging", planes: { messages: "capture" } }))).toEqual([
      "messages"
    ]);
  });
});

describe("oltpRecutQueued and settling", () => {
  const branch = {
    mode: "branch",
    status: "ready",
    fork_at: "2026-11-01T00:00:00Z",
    expires_at: "2026-12-01T00:00:00Z",
    error: null,
    recut_queued: false
  } as const;

  it("reads a reset the worker has not started from the server", () => {
    const settled = env({ oltp_branch: { ...branch } });
    const queued = env({ oltp_branch: { ...branch, recut_queued: true } });
    expect(oltpRecutQueued(settled)).toBe(false);
    expect(oltpRecutQueued(queued)).toBe(true);
    expect(settling(settled)).toBe(false);
    expect(settling(queued)).toBe(true);
  });

  it("does not take fork points that differ for a queued re-cut", () => {
    // An empty branch whose sandbox turned on airhouse=branch: the sandbox's
    // clock moved, and nothing is queued.
    const e = env({
      fork_at: "2026-11-04T09:00:00Z",
      oltp_branch: { ...branch, mode: "empty", expires_at: null }
    });
    expect(oltpRecutQueued(e)).toBe(false);
    expect(settling(e)).toBe(false);
  });

  it("polls while a branch moves or a sandbox is deleted, and not once it has failed", () => {
    for (const status of ["cutting", "migrating", "resetting"]) {
      expect(settling(env({ oltp_branch: { ...branch, status } }))).toBe(true);
    }
    const failed = env({
      oltp_branch: { ...branch, status: "failed", error: "boom", recut_queued: true }
    });
    expect(oltpRecutQueued(failed)).toBe(false);
    expect(settling(failed)).toBe(false);
    expect(settling(env({ status: "deleting" }))).toBe(true);
    expect(settling(env())).toBe(false);
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

describe("resetCopy", () => {
  it("says an empty branch comes back without production's rows", () => {
    expect(resetCopy("oltp", env({ planes: { oltp: "branch" } }))).toContain("from production");
    const empty = resetCopy("oltp", env({ planes: { oltp: "empty" } }));
    expect(empty).toContain("none of their rows");
    expect(empty).not.toContain("from production now");
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
