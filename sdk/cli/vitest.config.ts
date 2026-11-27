import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      // The SDK's manifest contract, from SOURCE — the same mapping as
      // `tsconfig.json` `paths` and `tsdown.config.ts` `alias`, for the reason
      // the first of those gives.
      "@oxy-hq/sdk/manifest": fileURLToPath(
        new URL("../typescript/src/manifest.ts", import.meta.url)
      ),
      // `oxyc fn dev`'s testing context, from source for the same reason.
      "@oxy-hq/sdk/testing": fileURLToPath(
        new URL("../typescript/src/testing/index.ts", import.meta.url)
      ),
      // The theme guard `oxyc checks run --build` runs, from source for the same reason.
      "@oxy-hq/sdk/theme-guard": fileURLToPath(
        new URL("../typescript/src/theme-guard.ts", import.meta.url)
      )
    }
  }
});
