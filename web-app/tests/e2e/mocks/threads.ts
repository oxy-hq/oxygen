import type { Page } from "@playwright/test";

/**
 * Thread Mocks for E2E Tests
 * -----------------------------------------------
 * Why we mock instead of calling the real /threads API:
 * 1. Avoid waiting for the language model / agent pipeline to finish answering.
 *    Real thread creation + message generation can introduce variable latency and
 *    sometimes transient failures, making tests slower and flaky.
 * 2. Provide a fully deterministic environment for assertions (stable IDs, titles,
 *    timestamps, pagination shape). This lets us verify UI rendering & navigation
 *    logic without coupling tests to backend state or async processing order.
 * Additional benefits:
 *    - Eliminates need for per-suite seeding & resets.
 *    - Allows precise simulation of list, detail, and messages endpoints with
 *      minimal payloads while preserving the response structure the frontend expects.
 * Route interception strategy:
 *    We intercept requests to the production-like base URL and fulfill them with
 *    static JSON. Any code that consumes fetch/React Query sees realistic data
 *    without exercising the actual persistence layer or LM.
 */

// Static deterministic thread dataset used for navigation & listing tests.
// Matches backend shape expected by the frontend (subset of fields).
interface MockThreadItem {
  id: string;
  title: string;
  input: string;
  output: string;
  source_type: string;
  source: string;
  created_at: string;
  references: unknown[];
  is_processing: boolean;
}

// Generate 10 identical mock threads with stable timestamps.
const NOW = new Date().toISOString();
const MOCK_THREADS: MockThreadItem[] = Array.from({ length: 10 }).map((_, i) => ({
  id: `00000000-0000-0000-0000-00000000000${i}`,
  title: "top 3 fruits",
  input: "top 3 fruits",
  output: "",
  source_type: "agent",
  source: "agents/duckdb.agentic.yml",
  created_at: NOW,
  references: [],
  is_processing: false
}));

const PROJECT_ID = "00000000-0000-0000-0000-000000000000";
const API_BASE = "http://localhost:3000/api";

// Minimal messages payload simulating answered thread
const MOCK_MESSAGES = [
  {
    id: "11111111-1111-1111-1111-111111111111",
    content: "top 3 fruits",
    is_human: true,
    thread_id: "", // will be replaced dynamically
    created_at: NOW,
    usage: { inputTokens: 0, outputTokens: 0 },
    run_info: {
      children: null,
      blocks: null,
      error: null,
      metadata: null,
      source_id: null,
      run_index: null,
      lookup_id: null,
      root_ref: null,
      status: null
    }
  },
  {
    id: "22222222-2222-2222-2222-222222222222",
    content:
      "\n| name | total_quantity |\n| --- | --- |\n| watermelon | 25 |\n| banana | 14 |\n| grape | 11 |",
    is_human: false,
    thread_id: "",
    created_at: NOW,
    usage: { inputTokens: 100, outputTokens: 20 },
    run_info: {
      children: null,
      blocks: null,
      error: null,
      metadata: null,
      source_id: null,
      run_index: null,
      lookup_id: null,
      root_ref: null,
      status: null
    }
  }
];

// Attach network route interceptions for threads list, single thread, messages.
export async function mockThreadsEndpoints(page: Page) {
  // List endpoint (pagination simplified to single page)
  await page.route(`${API_BASE}/${PROJECT_ID}/threads?*`, async (route) => {
    const json = {
      threads: MOCK_THREADS,
      pagination: {
        page: 1,
        limit: 100,
        total: MOCK_THREADS.length,
        total_pages: 1,
        has_next: false,
        has_previous: false
      }
    };
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify(json)
    });
  });

  // Individual thread endpoint
  await page.route(
    /http:\/\/localhost:3000\/api\/00000000-0000-0000-0000-000000000000\/threads\/00000000-0000-0000-0000-00000000000\d/,
    async (route) => {
      const url = route.request().url();
      const id = url.split("/").pop()!;
      const thread = MOCK_THREADS.find((t) => t.id === id);
      if (!thread) {
        await route.fulfill({ status: 404 });
        return;
      }
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(thread)
      });
    }
  );

  // Messages endpoint (returns 2 messages referencing the thread)
  await page.route(
    /http:\/\/localhost:3000\/api\/00000000-0000-0000-0000-000000000000\/threads\/00000000-0000-0000-0000-00000000000\d\/messages/,
    async (route) => {
      const parts = route.request().url().split("/");
      const threadId = parts[parts.length - 2];
      const messages = MOCK_MESSAGES.map((m) => ({
        ...m,
        thread_id: threadId
      }));
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(messages)
      });
    }
  );
}
