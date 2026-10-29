/**
 * The notification catalogue's checks are the SDK's, taken through
 * `@oxy-hq/sdk/manifest` — ONE module, not a byte copy. This package used to
 * carry `notification-catalog.ts` beside this test, kept in step by a sync
 * script; what this pins now is that the function `pnpm dev` and `pnpm build`
 * run IS the SDK's export, so an edit to the rules reaches every tool through
 * one import rather than three files.
 */

import { checkNotifications, checkWidgets } from "@oxy-hq/sdk/manifest";
import { describe, expect, it } from "vitest";
import { checkNotifications as sdkSource } from "../../typescript/src/custom-app/notification-catalog";
import { checkWidgets as widgetsSource } from "../../typescript/src/custom-app/widgets";
import { validateManifest } from "./index.js";

describe("the notification catalogue's checks", () => {
  it("are the SDK's own export, not a copy", () => {
    expect(checkNotifications).toBe(sdkSource);
  });

  it("run inside the plugin's manifest validation", () => {
    expect(
      validateManifest({
        schemaVersion: 2,
        slug: "x",
        functions: { f: { notify: { kinds: { "oxy.anomaly": {} } } } }
      })
    ).toEqual([expect.stringMatching(/^functions\.f\.notify\.kinds\.oxy\.anomaly: /)]);
  });
});

describe("the widget checks", () => {
  it("are the SDK's own export, not a copy", () => {
    expect(checkWidgets).toBe(widgetsSource);
  });

  it("run inside the plugin's manifest validation", () => {
    expect(
      validateManifest({
        schemaVersion: 2,
        slug: "x",
        functions: { f: {} },
        widgets: { w: { title: "W", function: "ghost" } }
      })
    ).toEqual([expect.stringMatching(/^widgets\.w\.function: /)]);
  });
});
