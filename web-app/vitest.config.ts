import path from "node:path";
import { defineConfig } from "vitest/config";

export default defineConfig({
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "src")
    }
  },
  test: {
    environment: "node",
    include: [
      "src/**/*.test.ts",
      "src/**/*.test.tsx",
      "tests/agentic/runner/**/*.test.ts",
      "tests/showcase/**/*.test.ts",
      // Build tooling whose output ships in every binary — see
      // vite-plugins/precompress.ts.
      "vite-plugins/**/*.test.ts"
    ],
    setupFiles: ["src/test-setup.ts"]
  }
});
