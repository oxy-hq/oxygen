import { execSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";

const database_path = "~/.local/share/oxy";
// (Global setup handles API seeding; no base URL or project ID needed here)

/**
 * Reset the project database.
 * NOTE: This should only be called from global-setup.ts, not from individual tests.
 * Tests should run against a single persistent database setup.
 */
export function resetProject() {
  // eslint-disable-next-line sonarjs/os-command
  execSync(`rm -rf ${database_path}`);
}

export async function resetTestFile() {
  // File should be in the examples directory of the oxygen-internal project
  const testFilePath = "../examples/test-file-for-e2e.txt";
  const originalContent = `# Test File for E2E Tests

This file is used for IDE E2E tests.
It gets modified during tests and reset after each test.
`;

  if (!existsSync("../examples")) {
    await mkdir("../examples", { recursive: true });
  }

  await writeFile(testFilePath, originalContent, "utf-8");
}

export async function resetTestAgentFile() {
  const testAgentPath = "../examples/agents/test-agent-e2e.agentic.yml";
  const originalContent = `# Test Agent for E2E Tests
# A test agent used for IDE E2E tests
llm:
  ref: openai-4o-mini
databases:
  - local
instructions: |
  You are a test agent used for E2E testing.
  This file gets modified during tests and reset after each test.
`;

  if (!existsSync("../examples/agents")) {
    await mkdir("../examples/agents", { recursive: true });
  }

  await writeFile(testAgentPath, originalContent, "utf-8");
}

export async function cleanupTestFiles() {
  const { unlink, readdir } = await import("node:fs/promises");
  const { existsSync } = await import("node:fs");
  const path = await import("node:path");

  const examplesDir = "../examples";

  if (!existsSync(examplesDir)) {
    return;
  }

  try {
    // Patterns to match test files
    const testPatterns = [
      /^test-create-\d+\.txt$/,
      /^nested-test-\d+\.txt$/,
      /^test-folder-\d+$/,
      /^test-escape-file\.txt$/,
      /^test-spaces\.txt$/,
      /^a{1000,}\.txt$/,
      /^test-error-file\.txt$/,
      /^test-network-file\.txt$/,
      /^test-renamed-\d+\.txt$/,
      /^renamed-.*\.txt$/
    ];

    // Delete files in root examples directory
    const files = await readdir(examplesDir);
    for (const file of files) {
      if (testPatterns.some((pattern) => pattern.test(file))) {
        const filePath = path.join(examplesDir, file);
        await unlink(filePath).catch(() => {});
        console.log(`Cleaned up: ${file}`);
      }
    }

    // Delete test files in automations subdirectory
    const workflowsDir = path.join(examplesDir, "workflows");
    if (existsSync(workflowsDir)) {
      const automationFiles = await readdir(workflowsDir);
      for (const file of automationFiles) {
        if (testPatterns.some((pattern) => pattern.test(file))) {
          const filePath = path.join(workflowsDir, file);
          await unlink(filePath).catch(() => {});
          console.log(`Cleaned up: workflows/${file}`);
        }
      }
    }
  } catch (error) {
    console.error("Error cleaning up test files:", error);
  }
}

// (Removed seedThreadsDataViaAPI to centralize seeding in global setup only)

// Create test threads via UI - slower but more realistic
