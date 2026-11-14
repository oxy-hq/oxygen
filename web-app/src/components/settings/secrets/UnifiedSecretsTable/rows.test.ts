import { describe, expect, it } from "vitest";
import type { Secret } from "@/types/secret";
import { buildRows, countByKind, filterRows, kindOf } from "./rows";
import type { UnifiedRow } from "./types";

const secret = (id: string, name: string, description?: string): Secret => ({
  id,
  name,
  description,
  created_at: "2026-11-01T00:00:00Z",
  updated_at: "2026-11-01T00:00:00Z",
  created_by: "user-1",
  is_active: true
});

const PULSE = "11111111-1111-1111-1111-111111111111";
const OPS = "22222222-2222-2222-2222-222222222222";
const names = new Map([
  [PULSE, "Platform Pulse"],
  [OPS, "Ops Bot"]
]);

const secrets = [
  secret("s-slack", "SLACK_BOT_TOKEN", "Bot for #daily-updates"),
  secret("s-claude", "ANTHROPIC_API_KEY"),
  secret("s-wh", "WAREHOUSE_PASSWORD"),
  secret("s-gh", `apps/${PULSE}/GITHUB_TOKEN`)
];
const shared = [{ secret_id: "s-slack", app_ids: [PULSE, OPS] }];

describe("buildRows", () => {
  const rows = buildRows(secrets, shared, names);

  it("puts workspace secrets first, then each app's own, by name", () => {
    expect(rows.map((r) => r.name)).toEqual([
      "ANTHROPIC_API_KEY",
      "SLACK_BOT_TOKEN",
      "WAREHOUSE_PASSWORD",
      "GITHUB_TOKEN"
    ]);
  });

  it("names the apps a workspace secret is shared with, alphabetically", () => {
    const slack = rows.find((r) => r.name === "SLACK_BOT_TOKEN") as UnifiedRow;
    expect(slack?.sharedWith.map((a) => a.name)).toEqual(["Ops Bot", "Platform Pulse"]);
    expect(kindOf(slack)).toBe("shared");
  });

  it("shows an app's own secret as the key it reads, attributed to the app", () => {
    const own = rows.find((r) => r.name === "GITHUB_TOKEN") as UnifiedRow;
    expect(own?.app).toEqual({ id: PULSE, name: "Platform Pulse" });
    expect(own?.sharedWith).toEqual([]);
    expect(kindOf(own)).toBe("app");
  });

  it("falls back to a short id for an app it cannot name", () => {
    const rows = buildRows(
      [secret("s", "K")],
      [{ secret_id: "s", app_ids: ["abcdef12-0000"] }],
      new Map()
    );
    expect(rows[0].sharedWith).toEqual([{ id: "abcdef12-0000", name: "abcdef12" }]);
  });
});

describe("filterRows", () => {
  const rows = buildRows(secrets, shared, names);

  it("keeps one kind at a time", () => {
    expect(filterRows(rows, "shared", "").map((r) => r.name)).toEqual(["SLACK_BOT_TOKEN"]);
    expect(filterRows(rows, "workspace", "").map((r) => r.name)).toEqual([
      "ANTHROPIC_API_KEY",
      "WAREHOUSE_PASSWORD"
    ]);
    expect(filterRows(rows, "app", "").map((r) => r.name)).toEqual(["GITHUB_TOKEN"]);
  });

  it("finds every secret an app reads by the app's name", () => {
    expect(filterRows(rows, "all", "pulse").map((r) => r.name)).toEqual([
      "SLACK_BOT_TOKEN",
      "GITHUB_TOKEN"
    ]);
  });

  it("matches a description, ignoring case and padding", () => {
    expect(filterRows(rows, "all", "  DAILY-updates ").map((r) => r.name)).toEqual([
      "SLACK_BOT_TOKEN"
    ]);
  });

  it("counts each kind", () => {
    expect(countByKind(rows)).toEqual({ all: 4, shared: 1, workspace: 2, app: 1 });
  });
});
