import { describe, expect, it } from "vitest";
import type { Secret } from "@/types/secret";
import {
  buildRows,
  countByApp,
  countByKind,
  filterRows,
  inScope,
  kindOf,
  readersOf,
  scopeOf
} from "./rows";
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
  secret("s-gh", `apps/${PULSE}/GITHUB_TOKEN`),
  secret("s-ops", `apps/${OPS}/GITHUB_TOKEN`)
];
const shared = [{ secret_id: "s-slack", app_ids: [PULSE, OPS] }];

describe("buildRows", () => {
  const rows = buildRows(secrets, shared, names);

  it("puts workspace secrets first, then each app's own, by name", () => {
    expect(rows.map((r) => r.name)).toEqual([
      "ANTHROPIC_API_KEY",
      "SLACK_BOT_TOKEN",
      "WAREHOUSE_PASSWORD",
      "GITHUB_TOKEN",
      "GITHUB_TOKEN"
    ]);
  });

  it("names the apps a workspace secret is shared with, alphabetically", () => {
    const slack = rows.find((r) => r.name === "SLACK_BOT_TOKEN") as UnifiedRow;
    expect(slack?.sharedWith.map((a) => a.name)).toEqual(["Ops Bot", "Platform Pulse"]);
    expect(kindOf(slack)).toBe("shared");
    expect(scopeOf(slack)).toBe("workspace");
  });

  it("shows an app's own secret as the key it reads, attributed to the app", () => {
    const own = rows.find((r) => r.app?.id === PULSE) as UnifiedRow;
    expect(own?.name).toBe("GITHUB_TOKEN");
    expect(own?.app).toEqual({ id: PULSE, name: "Platform Pulse" });
    expect(own?.sharedWith).toEqual([]);
    expect(scopeOf(own)).toBe("app");
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

/** Each tab is one scope's rows; nothing is in both. */
describe("inScope", () => {
  const rows = buildRows(secrets, shared, names);

  it("splits the two tabs and loses nothing", () => {
    const workspace = inScope(rows, "workspace");
    const app = inScope(rows, "app");
    expect(workspace.map((r) => r.name)).toEqual([
      "ANTHROPIC_API_KEY",
      "SLACK_BOT_TOKEN",
      "WAREHOUSE_PASSWORD"
    ]);
    expect(app.map((r) => r.app?.name)).toEqual(["Ops Bot", "Platform Pulse"]);
    expect(workspace.length + app.length).toBe(rows.length);
  });
});

describe("filterRows", () => {
  const workspace = inScope(buildRows(secrets, shared, names), "workspace");
  const appRows = inScope(buildRows(secrets, shared, names), "app");

  it("keeps the workspace secrets apps read, or the ones they don't", () => {
    expect(filterRows(workspace, { filter: "shared" }, "").map((r) => r.name)).toEqual([
      "SLACK_BOT_TOKEN"
    ]);
    expect(filterRows(workspace, { filter: "unshared" }, "").map((r) => r.name)).toEqual([
      "ANTHROPIC_API_KEY",
      "WAREHOUSE_PASSWORD"
    ]);
  });

  it("keeps one app's own secrets", () => {
    expect(filterRows(appRows, { appId: PULSE }, "").map((r) => r.app?.name)).toEqual([
      "Platform Pulse"
    ]);
    expect(filterRows(appRows, {}, "").length).toBe(2);
  });

  it("finds every workspace secret an app reads by the app's name", () => {
    expect(filterRows(workspace, { filter: "all" }, "pulse").map((r) => r.name)).toEqual([
      "SLACK_BOT_TOKEN"
    ]);
  });

  it("matches a description, ignoring case and padding", () => {
    expect(filterRows(workspace, { filter: "all" }, "  DAILY-updates ").map((r) => r.name)).toEqual(
      ["SLACK_BOT_TOKEN"]
    );
  });

  it("counts the workspace tab's two kinds, and each app's keys", () => {
    expect(countByKind(workspace)).toEqual({ all: 3, shared: 1, unshared: 2 });
    expect(countByApp(appRows)).toEqual(
      new Map([
        [PULSE, 1],
        [OPS, 1]
      ])
    );
  });
});

/** What deleting a row would take away, which is what its dialog names. */
describe("readersOf", () => {
  const rows = buildRows(secrets, shared, names);

  it("names the apps that read a shared secret", () => {
    const slack = rows.find((r) => r.name === "SLACK_BOT_TOKEN") as UnifiedRow;
    expect(readersOf(slack)).toEqual(["Ops Bot", "Platform Pulse"]);
  });

  it("names the one app that owns its own secret", () => {
    const own = rows.find((r) => r.app?.id === PULSE) as UnifiedRow;
    expect(readersOf(own)).toEqual(["Platform Pulse"]);
  });

  it("names nobody for a workspace secret no app reads", () => {
    const lonely = rows.find((r) => r.name === "WAREHOUSE_PASSWORD") as UnifiedRow;
    expect(readersOf(lonely)).toEqual([]);
  });
});
