import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

/**
 * The mark cutter lives in the SDK (`sdk/typescript/src/shell/railGlyph.ts`), which
 * draws the rail inside every custom app; the web-app keeps a byte-identical copy
 * because it does not depend on that package. One cutter, so an app's mark is the
 * same shape in Oxygen's rail and in the app's own.
 */
describe("railGlyph.ts", () => {
  it("is the SDK's file, byte for byte", () => {
    const here = readFileSync(resolve(import.meta.dirname, "railGlyph.ts"), "utf8");
    const sdk = readFileSync(
      resolve(import.meta.dirname, "../../../../sdk/typescript/src/shell/railGlyph.ts"),
      "utf8"
    );
    expect(here).toBe(sdk);
  });
});
