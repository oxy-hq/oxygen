import { describe, expect, it } from "vitest";
import type { Secret } from "@/types/secret";
import type { SharedSecretsResponse } from "@/types/sharedSecrets";
import { missingByApp } from "./missing";

const secret = (id: string, name: string): Secret => ({
  id,
  name,
  created_at: "2026-11-01T00:00:00Z",
  updated_at: "2026-11-01T00:00:00Z",
  created_by: "user-1",
  is_active: true
});

const sharing: SharedSecretsResponse = {
  shared: [{ secret_id: "s-slack", app_ids: ["ops"] }],
  apps: [
    {
      app_id: "pulse",
      app_name: "Platform Pulse",
      app_slug: "platform-pulse",
      missing: [
        { key: "SLACK_BOT_TOKEN", required: false },
        { key: "GITHUB_TOKEN", required: true },
        { key: "ANTHROPIC_API_KEY", required: false }
      ]
    },
    { app_id: "ops", app_name: "Ops Bot", app_slug: "ops-bot", missing: [] }
  ]
};

describe("missingByApp", () => {
  const secrets = [
    secret("s-slack", "SLACK_BOT_TOKEN"),
    secret("s-other", "apps/ops/ANTHROPIC_API_KEY")
  ];
  const [pulse, ...rest] = missingByApp(sharing, secrets);

  it("lists only apps that lack something", () => {
    expect(rest).toEqual([]);
    expect(pulse.appName).toBe("Platform Pulse");
  });

  it("puts required keys first, then by name", () => {
    expect(pulse.keys.map((k) => k.key)).toEqual([
      "GITHUB_TOKEN",
      "ANTHROPIC_API_KEY",
      "SLACK_BOT_TOKEN"
    ]);
  });

  it("offers the workspace secret of the same name, keeping who it is shared with", () => {
    expect(pulse.keys.find((k) => k.key === "SLACK_BOT_TOKEN")?.workspaceSecret).toEqual({
      id: "s-slack",
      appIds: ["ops"]
    });
  });

  it("never offers another app's own secret", () => {
    expect(pulse.keys.find((k) => k.key === "ANTHROPIC_API_KEY")?.workspaceSecret).toBeUndefined();
  });

  it("says nothing before the sharing view has loaded", () => {
    expect(missingByApp(undefined, secrets)).toEqual([]);
  });
});
