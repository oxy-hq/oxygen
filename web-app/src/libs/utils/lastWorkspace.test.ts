// @vitest-environment jsdom

import { beforeEach, describe, expect, it } from "vitest";
import type { WorkspaceSummary } from "@/services/api/workspaces";
import { pickWorkspace, setLastWorkspaceId } from "./lastWorkspace";

const ORG = "org-1";

const ws = (
  id: string,
  status: WorkspaceSummary["status"] = "ready",
  last_opened_at: string | null = null
): WorkspaceSummary => ({
  id,
  org_id: ORG,
  name: id,
  path: null,
  created_at: "2026-01-01T00:00:00Z",
  last_opened_at,
  created_by_name: null,
  status,
  error: null,
  agent_count: null,
  workflow_count: null,
  git_remote: null,
  git_commit: null,
  git_updated_at: null
});

const counts = (entries: Record<string, number>) => new Map(Object.entries(entries));

beforeEach(() => {
  localStorage.clear();
  // The remembered id is keyed by the signed-in user.
  localStorage.setItem("user", JSON.stringify({ id: "user-1" }));
});

describe("pickWorkspace", () => {
  it("lands on the workspace with the most apps when nothing is remembered", () => {
    // The prod shape: the API listed an app-less workspace first, and HQ opened empty.
    const list = [ws("pokehouse"), ws("oxygen"), ws("sandbox")];
    expect(pickWorkspace(list, ORG, counts({ pokehouse: 0, oxygen: 3, sandbox: 1 }))?.id).toBe(
      "oxygen"
    );
  });

  it("keeps the remembered workspace even when another has more apps", () => {
    setLastWorkspaceId(ORG, "pokehouse");
    const list = [ws("pokehouse"), ws("oxygen")];
    expect(pickWorkspace(list, ORG, counts({ pokehouse: 0, oxygen: 3 }))?.id).toBe("pokehouse");
  });

  it("ignores a remembered workspace that is gone", () => {
    setLastWorkspaceId(ORG, "deleted");
    const list = [ws("a"), ws("b")];
    expect(pickWorkspace(list, ORG, counts({ b: 2 }))?.id).toBe("b");
  });

  it("ignores a remembered workspace that is still cloning", () => {
    setLastWorkspaceId(ORG, "a");
    const list = [ws("a", "cloning"), ws("b")];
    expect(pickWorkspace(list, ORG, counts({ a: 5 }))?.id).toBe("b");
  });

  it("breaks a tie on apps by last opened, then by list order", () => {
    const list = [
      ws("first", "ready", null),
      ws("opened", "ready", "2026-10-01T00:00:00Z"),
      ws("second", "ready", null)
    ];
    expect(pickWorkspace(list, ORG, counts({ first: 2, opened: 2, second: 2 }))?.id).toBe("opened");
    expect(pickWorkspace([list[0], list[2]], ORG, counts({ first: 2, second: 2 }))?.id).toBe(
      "first"
    );
  });

  it("reads a workspace it could not count as zero apps", () => {
    const list = [ws("uncounted"), ws("counted")];
    expect(pickWorkspace(list, ORG, counts({ counted: 1 }))?.id).toBe("counted");
  });

  it("keeps the list order when there are no counts at all", () => {
    expect(pickWorkspace([ws("a"), ws("b")], ORG)?.id).toBe("a");
  });

  it("skips cloning workspaces but keeps failed ones", () => {
    const list = [ws("cloning", "cloning"), ws("failed", "failed")];
    expect(pickWorkspace(list, ORG, counts({ cloning: 9 }))?.id).toBe("failed");
  });

  it("returns null when nothing is navigable", () => {
    expect(pickWorkspace([ws("a", "cloning")], ORG, counts({ a: 1 }))).toBeNull();
  });
});
