import { existsSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { resetProject } from "./utils";

async function globalSetup() {
  console.log("Setting up test files for E2E tests...");

  const testFilePath = "../examples/test-file-for-e2e.txt";
  const testFileContent = `# Test File for E2E Tests

This file is used for IDE E2E tests.
It gets modified during tests and reset after each test.
`;

  const testAgentPath = "../examples/agents/test-agent-e2e.agentic.yml";
  const testAgentContent = `# Test Agent for E2E Tests
# A test agent used for IDE E2E tests
llm:
  ref: openai-4o-mini
databases:
  - local
instructions: |
  You are a test agent used for E2E testing.
  This file gets modified during tests and reset after each test.
`;

  try {
    // Reset database/storage before creating files & seeding threads
    resetProject();
    await writeFile(testFilePath, testFileContent, "utf-8");
    console.log(`✓ Created ${testFilePath}`);

    if (!existsSync("../examples/agents")) {
      await mkdir("../examples/agents", { recursive: true });
    }

    await writeFile(testAgentPath, testAgentContent, "utf-8");
    console.log(`✓ Created ${testAgentPath}`);

    // Give the backend file watcher a moment to detect the new files
    await new Promise((resolve) => setTimeout(resolve, 1500));

    console.log("Global E2E setup complete (mocked threads handled via route interception).");
  } catch (error) {
    console.error("Failed to setup test files:", error);
    throw error;
  }
}

export default globalSetup;
