/**
 * The notification catalogue's checks are the SDK's, taken through
 * `@oxy-hq/sdk/manifest` — ONE module, not a byte copy. This package used to
 * carry `notification-catalog.ts` beside this test, kept in step by a sync
 * script; what this pins now is that the function `oxyc validate` and `oxyc
 * publish` run IS the SDK's export, so an edit to the rules reaches every
 * tool through one import rather than three files.
 */

import { checkNotifications, checkWidgets, validateOxyAppManifest } from "@oxy-hq/sdk/manifest";
import { describe, expect, it } from "vitest";
import { checkNotifications as sdkSource } from "../../../typescript/src/custom-app/notification-catalog";
import { checkWidgets as widgetsSource } from "../../../typescript/src/custom-app/widgets";

describe("the notification catalogue's checks", () => {
  it("are the SDK's own export, not a copy", () => {
    expect(checkNotifications).toBe(sdkSource);
  });

  it("run inside the manifest validator the CLI uses", () => {
    const issues = validateOxyAppManifest({
      schemaVersion: 2,
      slug: "x",
      functions: { f: { notify: { kinds: { "oxy.anomaly": {} } } } }
    });
    expect(issues.map((i) => i.path)).toEqual(["functions.f.notify.kinds.oxy.anomaly"]);
  });
});

describe("the widget checks", () => {
  it("are the SDK's own export, not a copy", () => {
    expect(checkWidgets).toBe(widgetsSource);
  });

  it("run inside the manifest validator the CLI uses", () => {
    const issues = validateOxyAppManifest({
      schemaVersion: 2,
      slug: "x",
      functions: { f: {} },
      widgets: { w: { title: "W", function: "ghost" } }
    });
    expect(issues.map((i) => [i.level, i.path])).toEqual([["error", "widgets.w.function"]]);
  });
});
