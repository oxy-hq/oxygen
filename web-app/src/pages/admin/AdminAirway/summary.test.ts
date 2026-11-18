// @vitest-environment jsdom
// (The narrowing helpers come from the API module, whose client reads `window` on import.)

import { describe, expect, it } from "vitest";
import type { AirwayConfigValues, AirwaySourceKindConfig } from "@/services/api/airwayConfig";
import { summarizeKinds, unrecognisedFields } from "./summary";

const row = (
  contract_policy: string | null,
  environment: string | null = null
): AirwayConfigValues => ({
  contract_policy,
  environment,
  updated_at: "2026-11-18T00:00:00Z"
});

const kind = (
  source_kind: string,
  global: AirwayConfigValues | null,
  overrideWorkspaces: string[] = []
): AirwaySourceKindConfig => ({
  source_kind,
  global,
  overrides: overrideWorkspaces.map((workspace_id) => ({
    workspace_id,
    workspace_name: null,
    values: row("permissive")
  }))
});

describe("unrecognisedFields", () => {
  it("is empty for an absent row, an unset row and known spellings", () => {
    expect(unrecognisedFields(null)).toEqual([]);
    expect(unrecognisedFields(undefined)).toEqual([]);
    expect(unrecognisedFields(row(null, null))).toEqual([]);
    expect(unrecognisedFields(row("require_declared", "sandbox"))).toEqual([]);
  });

  it("names each stored field this build cannot read", () => {
    expect(unrecognisedFields(row("strict", "staging"))).toEqual([
      'Contract policy = "strict"',
      'Environment = "staging"'
    ]);
    expect(unrecognisedFields(row("permissive", "staging"))).toEqual(['Environment = "staging"']);
  });
});

describe("summarizeKinds", () => {
  it("counts nothing on a fresh deployment where every kind runs the default", () => {
    expect(summarizeKinds([kind("toast", null), kind("weather", null)])).toEqual({
      kinds: 2,
      configured: 0,
      tightened: 0,
      sandbox: 0,
      overrides: 0,
      overrideWorkspaces: 0,
      unrecognised: 0
    });
  });

  it("does not call an explicit permissive row tightened, but does call it configured", () => {
    const s = summarizeKinds([kind("toast", row("permissive"))]);
    expect(s.configured).toBe(1);
    expect(s.tightened).toBe(0);
  });

  it("counts both stricter policies as tightened, and sandbox separately", () => {
    const s = summarizeKinds([
      kind("toast", row("forbid_opaque")),
      kind("quickbooks", row("require_declared", "sandbox")),
      kind("weather", row(null, "sandbox")),
      kind("rest_api", null)
    ]);
    expect(s.configured).toBe(3);
    expect(s.tightened).toBe(2);
    expect(s.sandbox).toBe(2);
  });

  it("counts override rows and the distinct workspaces they belong to", () => {
    const s = summarizeKinds([
      kind("toast", null, ["ws-a", "ws-b"]),
      kind("rest_api", null, ["ws-a"])
    ]);
    expect(s.overrides).toBe(3);
    expect(s.overrideWorkspaces).toBe(2);
  });

  it("counts an unreadable spelling as unrecognised, never as tightened or sandbox", () => {
    const s = summarizeKinds([kind("toast", row("strict", "staging"))]);
    expect(s.unrecognised).toBe(1);
    expect(s.tightened).toBe(0);
    expect(s.sandbox).toBe(0);
    expect(s.configured).toBe(1);
  });
});
