/**
 * The `design_system` check with the SDK's REAL theme guard, as the bundle
 * carries it (`@oxy-hq/sdk/theme-guard`, aliased to the SDK's source).
 *
 * `design-system.test.ts` stands in for the guard; this is what proves the
 * check reaches the real one and reads built CSS: a Tailwind variant class
 * (`.dark\:bg-card`) is a finding in source mode and none in built mode, so a
 * guard run without `{ built: true }` fails here.
 */

import { describe, expect, it } from "vitest";
import { checkDesignSystem } from "./design-system.js";

const BUILT = [
  // What Tailwind compiles `dark:bg-card` into: a class NAMED `dark:bg-card`.
  ".dark\\:bg-card:is(.dark *){background:var(--card)}",
  // An app restating a token the SDK theme owns.
  ".app-shell{--card:#fff}"
].join("\n");

describe("checkDesignSystem with the SDK's guard", () => {
  it("reads built CSS: a variant class is no bare .dark, a restated token still is a finding", () => {
    const out = checkDesignSystem(
      [{ file: "assets/index-abc.css", css: BUILT }],
      "2.31.0",
      "2.30.0"
    );
    expect(out.detail.findings).toEqual([
      {
        severity: "error",
        message: "restates --card, which theme.css defines; remove it",
        at: "assets/index-abc.css:2",
        rule: "restates-sdk-token"
      }
    ]);
    expect(out.status).toBe("failed");
    expect(out.summary).toBe("assets/index-abc.css:2 restates --card");
  });

  it("reads the <style> blocks of index.html, and passes a clean build", () => {
    const html =
      "<!doctype html>\n<html><head>\n<style>.app{color:var(--foreground)}</style>\n</head></html>";
    const out = checkDesignSystem(
      [
        { file: "index.html", css: html },
        { file: "assets/index-abc.css", css: ".app{background:var(--card)}" }
      ],
      "2.31.0",
      "2.30.0"
    );
    expect(out.status).toBe("passed");
    expect(out.detail.files).toEqual(["index.html", "assets/index-abc.css"]);
  });
});
