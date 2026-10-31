import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

/**
 * Oxygen and every custom app move alike because they load the same file: the
 * SDK's motion.css (`@oxy-hq/sdk/motion.css`, imported by the SDK shell). The
 * web-app keeps a copy, since the fleet image builds it without the SDK's
 * sources. This fails the moment the two differ.
 */
const here = path.dirname(fileURLToPath(import.meta.url));
const sdkMotion = path.resolve(here, "../../../../sdk/typescript/src/motion/motion.css");
const read = (file: string) => readFileSync(file, "utf8");

describe("motion", () => {
  it("is the SDK's motion language, byte for byte", () => {
    expect(
      read(path.join(here, "sdk-motion.css")),
      "web-app/src/styles/shadcn/sdk-motion.css differs from sdk/typescript/src/motion/motion.css: edit the SDK file, then copy it over"
    ).toBe(read(sdkMotion));
  });

  it("times Oxygen's own motion from the shared tokens only", () => {
    const local = read(path.join(here, "motion.css")).replace(/\/\*[\s\S]*?\*\//g, "");
    const defined = new Set([...read(sdkMotion).matchAll(/(--oxy-[a-z-]+):/g)].map((m) => m[1]));
    for (const [, token] of local.matchAll(/var\((--oxy-[a-z-]+)\)/g)) {
      expect(defined).toContain(token);
    }
    expect(local).not.toMatch(/\b\d+m?s\b|cubic-bezier/);
  });
});
