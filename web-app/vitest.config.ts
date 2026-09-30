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
      "tests/showcase/**/*.test.ts"
    ],
    setupFiles: ["src/test-setup.ts"]
  }
});
