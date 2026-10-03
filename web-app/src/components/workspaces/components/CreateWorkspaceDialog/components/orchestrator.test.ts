import { describe, expect, it } from "vitest";
import { deriveMessages, deriveRailState, initialState, reducer } from "./orchestrator";
import type { GithubSetup, OnboardingState } from "./types";

/**
 * The right-rail expected-file list tells the user which artifacts the
 * onboarding builder will create. After migrating the analytics agent from
 * `.agent.yml` (classic) to `.agentic.yml` (multi-step FSM pipeline), the rail
 * must advertise the new file name and type so the download checklist matches
 * what the builder actually writes.
 */
describe("deriveRailState — expected files", () => {
  const baseWithTables: OnboardingState = {
    ...initialState,
    step: "building",
    selectedTables: ["public.orders", "public.customers"]
  };

  it("advertises analytics.agentic.yml as the agent phase output", () => {
    const rail = deriveRailState(baseWithTables);
    const agenticEntry = rail.expectedFiles.find((f) => f.type === "agentic");
    expect(agenticEntry).toEqual({
      name: "analytics.agentic.yml",
      type: "agentic"
    });
  });

  it("no longer advertises a legacy .agent.yml expected file", () => {
    const rail = deriveRailState(baseWithTables);
    expect(rail.expectedFiles.find((f) => f.type === "agent")).toBeUndefined();
    expect(rail.expectedFiles.find((f) => f.name.endsWith(".agent.yml"))).toBeUndefined();
  });

  it("preserves config, per-table view, and app entries around the agentic entry", () => {
    const rail = deriveRailState(baseWithTables);
    // The second-app entry is typed "app2", not "app": its filename can't be
    // predicted (single-topic or cross-topic dashboard), so the rail matches it
    // against any non-overview `.app.yml`. Its `name` is only a display label —
    // the second topic alphabetically (here: "orders" from ["customers",
    // "orders"]), title-cased — shown until the real `title:` is read.
    expect(rail.expectedFiles).toEqual([
      { name: "config.yml", type: "config" },
      { name: "orders", type: "view" },
      { name: "customers", type: "view" },
      { name: "analytics.agentic.yml", type: "agentic" },
      { name: "overview", type: "app" },
      { name: "Orders", type: "app2" }
    ]);
  });

  it("title-cases a snake_case topic for the second-app label", () => {
    const rail = deriveRailState({
      ...initialState,
      step: "building",
      selectedTables: ["public.customers", "public.order_items"]
    });
    expect(rail.expectedFiles.find((f) => f.type === "app2")).toEqual({
      name: "Order Items",
      type: "app2"
    });
  });

  it("omits the second-app entry when the workspace has only one topic", () => {
    const singleTopic: OnboardingState = {
      ...initialState,
      step: "building",
      selectedTables: ["public.orders"]
    };
    const rail = deriveRailState(singleTopic);
    const appEntries = rail.expectedFiles.filter((f) => f.type === "app" || f.type === "app2");
    expect(appEntries).toEqual([{ name: "overview", type: "app" }]);
  });

  it("returns an empty expected-files list until tables are selected", () => {
    const rail = deriveRailState({ ...initialState, step: "building" });
    expect(rail.expectedFiles).toEqual([]);
  });
});

/**
 * The github / demo flows render one prompt at a time. The loop in
 * `deriveGithubMessages` used to render every missing key (and warehouse) up
 * front — including upcoming ones with no input field — which made it look
 * like the assistant was asking for several keys at once. Confirm that while
 * the flow is active we only emit messages up to and including the cursor.
 */
describe("deriveMessages — one prompt at a time", () => {
  const setupTwoKeys: GithubSetup = {
    missing_llm_key_vars: [
      { var_name: "OPENAI_API_KEY", vendor: "OpenAI" },
      { var_name: "ANTHROPIC_API_KEY", vendor: "Anthropic" }
    ],
    warehouses: []
  };

  it("does not preview upcoming LLM key prompts while the flow is active", () => {
    const state: OnboardingState = {
      ...initialState,
      mode: "github",
      step: "github_llm_keys",
      githubSetup: setupTwoKeys,
      githubLlmKeyCursor: 0
    };
    const messages = deriveMessages(state);
    expect(messages.find((m) => m.id === "github_llm_key_OPENAI_API_KEY")).toBeDefined();
    expect(messages.find((m) => m.id === "github_llm_key_ANTHROPIC_API_KEY")).toBeUndefined();
  });

  it("renders completed and active LLM key prompts but not the next one", () => {
    const setupThree: GithubSetup = {
      missing_llm_key_vars: [
        { var_name: "OPENAI_API_KEY", vendor: "OpenAI" },
        { var_name: "ANTHROPIC_API_KEY", vendor: "Anthropic" },
        { var_name: "GEMINI_API_KEY", vendor: "Google" }
      ],
      warehouses: []
    };
    const state: OnboardingState = {
      ...initialState,
      mode: "github",
      step: "github_llm_keys",
      githubSetup: setupThree,
      githubLlmKeyCursor: 1
    };
    const messages = deriveMessages(state);
    expect(messages.find((m) => m.id === "github_llm_key_OPENAI_API_KEY")).toBeDefined();
    expect(messages.find((m) => m.id === "github_llm_key_ANTHROPIC_API_KEY")).toBeDefined();
    expect(messages.find((m) => m.id === "github_llm_key_GEMINI_API_KEY")).toBeUndefined();
  });

  it("renders all completed LLM keys as history once the step has advanced", () => {
    const state: OnboardingState = {
      ...initialState,
      mode: "github",
      step: "complete",
      githubSetup: setupTwoKeys,
      githubLlmKeyCursor: 2
    };
    const messages = deriveMessages(state);
    expect(messages.find((m) => m.id === "github_llm_key_OPENAI_API_KEY")).toBeDefined();
    expect(messages.find((m) => m.id === "github_llm_key_ANTHROPIC_API_KEY")).toBeDefined();
  });

  it("renders LLM keys as history while the warehouse step is active", () => {
    // Intermediate state: every LLM key is done, but the user is now answering
    // warehouse credentials. Without this guard, a regression that gates LLM
    // history on `step === "complete"` would silently hide previously-saved
    // keys mid-flow.
    const setupKeysAndWarehouse: GithubSetup = {
      missing_llm_key_vars: setupTwoKeys.missing_llm_key_vars,
      warehouses: [
        {
          name: "warehouse_a",
          dialect: "postgres",
          missing_vars: [{ field: "password", var_name: "WAREHOUSE_A_PASSWORD", required: true }]
        }
      ]
    };
    const state: OnboardingState = {
      ...initialState,
      mode: "github",
      step: "github_warehouse_creds",
      githubSetup: setupKeysAndWarehouse,
      githubLlmKeyCursor: 2,
      githubWarehouseCursor: 0
    };
    const messages = deriveMessages(state);
    const openai = messages.find((m) => m.id === "github_llm_key_OPENAI_API_KEY");
    const anthropic = messages.find((m) => m.id === "github_llm_key_ANTHROPIC_API_KEY");
    expect(openai?.status).toBe("complete");
    expect(anthropic?.status).toBe("complete");
  });

  it("does not preview upcoming warehouse credential forms while the flow is active", () => {
    const setupTwoWarehouses: GithubSetup = {
      missing_llm_key_vars: [],
      warehouses: [
        {
          name: "warehouse_a",
          dialect: "postgres",
          missing_vars: [{ field: "password", var_name: "WAREHOUSE_A_PASSWORD", required: true }]
        },
        {
          name: "warehouse_b",
          dialect: "postgres",
          missing_vars: [{ field: "password", var_name: "WAREHOUSE_B_PASSWORD", required: true }]
        }
      ]
    };
    const state: OnboardingState = {
      ...initialState,
      mode: "github",
      step: "github_warehouse_creds",
      githubSetup: setupTwoWarehouses,
      githubWarehouseCursor: 0
    };
    const messages = deriveMessages(state);
    expect(messages.find((m) => m.id === "github_warehouse_warehouse_a")).toBeDefined();
    expect(messages.find((m) => m.id === "github_warehouse_warehouse_b")).toBeUndefined();
  });

  it("applies the same one-at-a-time behavior in demo mode", () => {
    // Demo and github modes share `deriveGithubMessages`, so the fix should
    // cover both. Pinning demo mode here guards against a future split where
    // demo gets its own renderer and the upcoming-prompt preview comes back.
    const state: OnboardingState = {
      ...initialState,
      mode: "demo",
      step: "github_llm_keys",
      githubSetup: setupTwoKeys,
      githubLlmKeyCursor: 0
    };
    const messages = deriveMessages(state);
    expect(messages.find((m) => m.id === "github_llm_key_OPENAI_API_KEY")).toBeDefined();
    expect(messages.find((m) => m.id === "github_llm_key_ANTHROPIC_API_KEY")).toBeUndefined();
  });
});

describe("reducer — INIT_GITHUB_FLOW", () => {
  it("seeds a fresh state into the github flow", () => {
    const next = reducer(
      { ...initialState, storageKey: "local:abc" },
      { type: "INIT_GITHUB_FLOW", mode: "demo" }
    );
    expect(next.mode).toBe("demo");
    expect(next.step).toBe("github_loading");
  });

  it("is idempotent — does not clobber a wizard that already chose a mode", () => {
    const inFlight: OnboardingState = {
      ...initialState,
      storageKey: "uuid-1",
      mode: "new",
      step: "warehouse_credentials"
    };
    const next = reducer(inFlight, { type: "INIT_GITHUB_FLOW", mode: "demo" });
    expect(next).toBe(inFlight);
  });

  it("is idempotent — does not clobber a wizard advanced past welcome", () => {
    const advanced: OnboardingState = {
      ...initialState,
      storageKey: "local:xyz",
      step: "github_llm_keys"
    };
    const next = reducer(advanced, { type: "INIT_GITHUB_FLOW", mode: "demo" });
    expect(next).toBe(advanced);
  });
});
