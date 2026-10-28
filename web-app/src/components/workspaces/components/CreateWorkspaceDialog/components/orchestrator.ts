import { useCallback, useEffect, useMemo, useReducer } from "react";
import {
  LEGACY_GLOBAL_KEY,
  type PersistableState,
  storageKey,
  VALID_STEPS
} from "@/libs/utils/onboardingStorage";
import type {
  BuildPhase,
  ConnectionStatus,
  GithubSetup,
  LlmProvider,
  OnboardingMessage,
  OnboardingMode,
  OnboardingRailState,
  OnboardingState,
  OnboardingStep,
  PhaseStatus,
  SchemaInfo,
  SelectionOption,
  WarehouseType
} from "./types";

// ── Actions ─────────────────────────────────────────────────────────────────

// Exported for unit tests; consumers use `useOnboardingOrchestrator`.
export type Action =
  | { type: "SET_LLM_PROVIDER"; provider: LlmProvider }
  | { type: "SET_LLM_MODEL"; model: string; modelRef: string; vendor: string }
  | { type: "SET_LLM_KEY"; apiKey: string }
  | { type: "START_LLM_KEY_TEST" }
  | { type: "FAIL_LLM_KEY_TEST"; error: string }
  | { type: "SET_WAREHOUSE_TYPE"; warehouseType: WarehouseType }
  | { type: "SET_WAREHOUSE_CREDENTIALS"; credentials: Record<string, string> }
  | { type: "SET_UPLOADED_WAREHOUSE_FILES"; files: string[]; subdir: string }
  | { type: "SET_CONNECTION_STATUS"; status: ConnectionStatus; error?: string }
  | { type: "SET_DISCOVERED_SCHEMAS"; schemas: SchemaInfo[] }
  | { type: "HYDRATE_DISCOVERED_SCHEMAS"; schemas: SchemaInfo[] }
  | {
      type: "SET_SCHEMA_TABLES_STATUS";
      schema: string;
      status: "loading" | "error";
      error?: string;
    }
  | {
      type: "SET_SCHEMA_TABLES";
      schema: string;
      tables: SchemaInfo["tables"];
    }
  | { type: "SET_SCHEMA_DISCOVERY_STATUS"; message: string | undefined }
  | { type: "SET_SCHEMA_DISCOVERY_ERROR"; error: string }
  | { type: "SET_SELECTED_TABLES"; tables: string[] }
  | { type: "START_PHASE"; phase: BuildPhase; threadId: string; runId: string }
  | { type: "SET_PHASE_STATUS"; phase: BuildPhase; status: PhaseStatus }
  | { type: "START_VIEW_RUN"; table: string; runId: string }
  | { type: "SET_VIEW_RUN_STATUS"; table: string; status: PhaseStatus }
  | { type: "SET_BUILD_ERROR"; error: string }
  | { type: "STOP_BUILD"; message: string }
  | { type: "COMPLETE"; createdFiles: string[]; sampleQuestions: string[] }
  | { type: "GO_TO_STEP"; step: OnboardingStep }
  // GitHub-flow actions
  | { type: "SET_GITHUB_SETUP"; setup: GithubSetup }
  | { type: "ADVANCE_GITHUB_LLM_KEY" }
  | {
      type: "ADVANCE_GITHUB_WAREHOUSE";
      result: "success" | "skipped" | "failed";
      warehouseName: string;
    }
  | { type: "START_GITHUB_WAREHOUSE_TEST" }
  | { type: "FAIL_GITHUB_WAREHOUSE_TEST"; error: string }
  | { type: "START_GITHUB_LLM_KEY_TEST" }
  | { type: "FAIL_GITHUB_LLM_KEY_TEST"; error: string }
  /** Idempotent: only mutates when `mode` is undefined and `step` is still
   * the initial `welcome`, so dispatching from a render-time effect won't
   * clobber an in-flight wizard. */
  | { type: "INIT_GITHUB_FLOW"; mode: OnboardingMode };

// ── Initial State ───────────────────────────────────────────────────────────

export const initialState: OnboardingState = {
  step: "welcome",
  connectionStatus: "idle",
  discoveredSchemas: [],
  selectedTables: []
};

// ── Reducer ─────────────────────────────────────────────────────────────────

// Exported for unit tests; consumers use `useOnboardingOrchestrator`.
export function reducer(state: OnboardingState, action: Action): OnboardingState {
  switch (action.type) {
    case "SET_LLM_PROVIDER":
      return { ...state, step: "llm_model", llmProvider: action.provider };

    case "SET_LLM_MODEL":
      return {
        ...state,
        step: "llm_key",
        llmModel: action.model,
        llmModelRef: action.modelRef,
        llmVendor: action.vendor
      };

    case "SET_LLM_KEY":
      return {
        ...state,
        step: "warehouse_type",
        llmApiKey: action.apiKey,
        llmKeyTesting: false,
        llmKeyError: undefined
      };

    case "START_LLM_KEY_TEST":
      return { ...state, llmKeyTesting: true, llmKeyError: undefined };

    case "FAIL_LLM_KEY_TEST":
      return { ...state, llmKeyTesting: false, llmKeyError: action.error };

    case "SET_WAREHOUSE_TYPE":
      return {
        ...state,
        step: "warehouse_credentials",
        warehouseType: action.warehouseType
      };

    case "SET_WAREHOUSE_CREDENTIALS":
      return {
        ...state,
        step: "connection_test",
        warehouseCredentials: action.credentials,
        connectionStatus: "testing"
      };

    case "SET_UPLOADED_WAREHOUSE_FILES":
      return {
        ...state,
        uploadedWarehouseFiles: action.files,
        uploadedWarehouseSubdir: action.subdir
      };

    case "SET_CONNECTION_STATUS":
      if (action.status === "success") {
        return {
          ...state,
          step: "schema_discovery",
          connectionStatus: "success",
          connectionError: undefined
        };
      }
      return {
        ...state,
        connectionStatus: action.status,
        connectionError: action.error
      };

    case "SET_DISCOVERED_SCHEMAS":
      return {
        ...state,
        step: "table_selection",
        discoveredSchemas: action.schemas,
        schemaDiscoveryError: undefined,
        schemaDiscoveryStatus: undefined
      };

    case "HYDRATE_DISCOVERED_SCHEMAS":
      // Update column metadata in-place after a filtered re-sync — does NOT
      // change `step`, so callers can refresh schemas mid-build without
      // bouncing the user back to the table picker.
      return { ...state, discoveredSchemas: action.schemas, schemaDiscoveryStatus: undefined };

    case "SET_SCHEMA_TABLES_STATUS":
      return {
        ...state,
        discoveredSchemas: state.discoveredSchemas.map((schema) =>
          schema.schema === action.schema
            ? {
                ...schema,
                loading: action.status === "loading",
                loadError: action.status === "error" ? action.error : undefined
              }
            : schema
        )
      };

    case "SET_SCHEMA_TABLES":
      return {
        ...state,
        discoveredSchemas: state.discoveredSchemas.map((schema) =>
          schema.schema === action.schema
            ? {
                ...schema,
                tables: action.tables,
                tableCount: action.tables.length || schema.tableCount,
                loaded: true,
                loading: false,
                loadError: undefined
              }
            : schema
        )
      };

    case "SET_SCHEMA_DISCOVERY_STATUS":
      return { ...state, schemaDiscoveryStatus: action.message };

    case "SET_SCHEMA_DISCOVERY_ERROR":
      return { ...state, schemaDiscoveryError: action.error, schemaDiscoveryStatus: undefined };

    case "SET_SELECTED_TABLES":
      return {
        ...state,
        step: "building",
        selectedTables: action.tables,
        builderThreadId: undefined,
        phaseRunIds: undefined,
        phaseStatuses: undefined,
        viewRunIds: undefined,
        viewRunStatuses: undefined,
        buildError: undefined,
        createdFiles: undefined,
        sampleQuestions: undefined
      };

    case "START_PHASE":
      return {
        ...state,
        // First phase creates the thread; subsequent phases reuse it
        builderThreadId: state.builderThreadId ?? action.threadId,
        phaseRunIds: { ...state.phaseRunIds, [action.phase]: action.runId },
        phaseStatuses: { ...state.phaseStatuses, [action.phase]: "running" as PhaseStatus },
        buildError: undefined
      };

    case "SET_PHASE_STATUS":
      return {
        ...state,
        phaseStatuses: { ...state.phaseStatuses, [action.phase]: action.status }
      };

    case "START_VIEW_RUN":
      return {
        ...state,
        viewRunIds: { ...state.viewRunIds, [action.table]: action.runId },
        viewRunStatuses: { ...state.viewRunStatuses, [action.table]: "running" as PhaseStatus }
      };

    case "SET_VIEW_RUN_STATUS":
      return {
        ...state,
        viewRunStatuses: { ...state.viewRunStatuses, [action.table]: action.status }
      };

    case "SET_BUILD_ERROR":
      return { ...state, buildError: action.error };

    case "STOP_BUILD": {
      // Flip any still-running phase / view to failed so the progress rail
      // and messages reflect the cancellation without relying on an in-flight
      // SSE to land the terminal event. Completed phases stay done.
      const nextPhaseStatuses: Partial<Record<BuildPhase, PhaseStatus>> = {
        ...(state.phaseStatuses ?? {})
      };
      for (const [phase, status] of Object.entries(nextPhaseStatuses) as [
        BuildPhase,
        PhaseStatus
      ][]) {
        if (status === "running") nextPhaseStatuses[phase] = "failed";
      }
      const nextViewRunStatuses: Record<string, PhaseStatus> = {
        ...(state.viewRunStatuses ?? {})
      };
      for (const [table, status] of Object.entries(nextViewRunStatuses)) {
        if (status === "running") nextViewRunStatuses[table] = "failed";
      }
      return {
        ...state,
        phaseStatuses: nextPhaseStatuses,
        viewRunStatuses: nextViewRunStatuses,
        buildError: action.message
      };
    }

    case "COMPLETE":
      return {
        ...state,
        step: "complete",
        createdFiles: action.createdFiles,
        sampleQuestions: action.sampleQuestions
      };

    case "SET_GITHUB_SETUP": {
      // First question / warehouse drives the step cursor. If nothing is
      // missing we skip straight to complete — the repo is fully configured.
      const hasLlmKeys = action.setup.missing_llm_key_vars.length > 0;
      const hasWarehouses = action.setup.warehouses.length > 0;
      const nextStep: OnboardingStep = hasLlmKeys
        ? "github_llm_keys"
        : hasWarehouses
          ? "github_warehouse_creds"
          : "complete";
      return {
        ...state,
        step: nextStep,
        githubSetup: action.setup,
        githubLlmKeyCursor: 0,
        githubWarehouseCursor: 0,
        githubWarehouseResults: {}
      };
    }

    case "ADVANCE_GITHUB_LLM_KEY": {
      const cursor = (state.githubLlmKeyCursor ?? 0) + 1;
      const total = state.githubSetup?.missing_llm_key_vars.length ?? 0;
      const hasWarehouses = (state.githubSetup?.warehouses.length ?? 0) > 0;
      // Always clear per-prompt validation state — the next entry has its
      // own key + provider, so the previous error / busy flag must not bleed
      // through to its initial render.
      if (cursor < total) {
        return {
          ...state,
          githubLlmKeyCursor: cursor,
          githubLlmKeyTesting: false,
          githubLlmKeyError: undefined
        };
      }
      return {
        ...state,
        githubLlmKeyCursor: cursor,
        githubLlmKeyTesting: false,
        githubLlmKeyError: undefined,
        step: hasWarehouses ? "github_warehouse_creds" : "complete"
      };
    }

    case "START_GITHUB_LLM_KEY_TEST":
      return { ...state, githubLlmKeyTesting: true, githubLlmKeyError: undefined };

    case "FAIL_GITHUB_LLM_KEY_TEST":
      return {
        ...state,
        githubLlmKeyTesting: false,
        githubLlmKeyError: action.error
      };

    case "ADVANCE_GITHUB_WAREHOUSE": {
      const cursor = (state.githubWarehouseCursor ?? 0) + 1;
      const total = state.githubSetup?.warehouses.length ?? 0;
      const results = {
        ...(state.githubWarehouseResults ?? {}),
        [action.warehouseName]: action.result
      };
      // Moving to the next warehouse always clears the active submission /
      // error state — each warehouse has its own form instance.
      if (cursor < total) {
        return {
          ...state,
          githubWarehouseCursor: cursor,
          githubWarehouseResults: results,
          githubWarehouseSubmitting: false,
          githubWarehouseError: undefined
        };
      }
      return {
        ...state,
        githubWarehouseCursor: cursor,
        githubWarehouseResults: results,
        githubWarehouseSubmitting: false,
        githubWarehouseError: undefined,
        step: "complete"
      };
    }

    case "START_GITHUB_WAREHOUSE_TEST":
      return {
        ...state,
        githubWarehouseSubmitting: true,
        githubWarehouseError: undefined
      };

    case "FAIL_GITHUB_WAREHOUSE_TEST":
      return {
        ...state,
        githubWarehouseSubmitting: false,
        githubWarehouseError: action.error
      };

    case "INIT_GITHUB_FLOW":
      // Idempotence guard — see action type doc.
      if (state.mode !== undefined || state.step !== "welcome") return state;
      return {
        ...state,
        mode: action.mode,
        step: "github_loading"
      };

    case "GO_TO_STEP": {
      const targetIdx = STEP_ORDER.indexOf(action.step);
      const currentIdx = STEP_ORDER.indexOf(state.step);

      // Going backwards — clear state from the target step onwards
      if (targetIdx < currentIdx) {
        const cleared: Partial<OnboardingState> = {};
        if (targetIdx <= stepIndex("llm_provider")) {
          cleared.llmProvider = undefined;
        }
        if (targetIdx <= stepIndex("llm_model")) {
          cleared.llmModel = undefined;
          cleared.llmModelRef = undefined;
          cleared.llmVendor = undefined;
        }
        if (targetIdx <= stepIndex("llm_key")) {
          cleared.llmApiKey = undefined;
          cleared.llmKeyTesting = false;
          cleared.llmKeyError = undefined;
        }
        if (targetIdx <= stepIndex("warehouse_type")) {
          cleared.warehouseType = undefined;
        }
        if (targetIdx <= stepIndex("warehouse_credentials")) {
          cleared.warehouseCredentials = undefined;
        }
        if (targetIdx <= stepIndex("warehouse_type")) {
          // Uploaded files are tied to the warehouse choice — clearing the
          // warehouse type implies the user is picking a different backend, so
          // the prior upload list is no longer meaningful. The files still live
          // on disk in `.db/`; "Start over" is the explicit way to delete them.
          cleared.uploadedWarehouseFiles = undefined;
          cleared.uploadedWarehouseSubdir = undefined;
        }
        if (targetIdx <= stepIndex("connection_test")) {
          cleared.connectionStatus = "idle" as ConnectionStatus;
          cleared.connectionError = undefined;
        }
        if (targetIdx <= stepIndex("schema_discovery")) {
          cleared.discoveredSchemas = [];
          cleared.schemaDiscoveryError = undefined;
          cleared.schemaDiscoveryStatus = undefined;
        }
        if (targetIdx <= stepIndex("table_selection")) {
          cleared.selectedTables = [];
          cleared.builderThreadId = undefined;
          cleared.phaseRunIds = undefined;
          cleared.phaseStatuses = undefined;
          cleared.viewRunIds = undefined;
          cleared.viewRunStatuses = undefined;
          cleared.buildError = undefined;
          cleared.createdFiles = undefined;
          cleared.sampleQuestions = undefined;
        }
        return { ...state, ...cleared, step: action.step };
      }

      // Going forward or same step — just set the step and clear relevant errors
      return {
        ...state,
        step: action.step,
        schemaDiscoveryError:
          action.step === "schema_discovery" ? undefined : state.schemaDiscoveryError,
        ...(action.step === "building"
          ? {
              buildError: undefined,
              builderThreadId: undefined,
              phaseRunIds: undefined,
              phaseStatuses: undefined,
              viewRunIds: undefined,
              viewRunStatuses: undefined,
              createdFiles: undefined,
              sampleQuestions: undefined
            }
          : {})
      };
    }

    default:
      return state;
  }
}

// ── Step Ordering ───────────────────────────────────────────────────────────

const STEP_ORDER: OnboardingStep[] = [
  "welcome",
  "llm_provider",
  "llm_model",
  "llm_key",
  "warehouse_type",
  "warehouse_credentials",
  "connection_test",
  "schema_discovery",
  "table_selection",
  "building",
  // GitHub-only steps — not on the blank-workspace path. Ordering is
  // informational; github mode skips directly between them.
  "github_loading",
  "github_llm_keys",
  "github_warehouse_creds",
  "github_connection_test",
  "complete"
];

function stepIndex(step: OnboardingStep): number {
  return STEP_ORDER.indexOf(step);
}

/** Steps that support going back, mapped to their previous step. */
const BACK_TARGETS: Partial<Record<OnboardingStep, OnboardingStep>> = {
  llm_model: "llm_provider",
  llm_key: "llm_model",
  warehouse_credentials: "warehouse_type",
  connection_test: "warehouse_credentials",
  schema_discovery: "warehouse_credentials",
  table_selection: "warehouse_credentials"
};

export function getPreviousStep(current: OnboardingStep): OnboardingStep | undefined {
  return BACK_TARGETS[current];
}

// ── GitHub mode: message derivation ─────────────────────────────────────────
//
// GitHub flow is shorter and cursor-driven: one prompt per missing LLM key,
// then one credential form per warehouse needing creds. Each answered prompt
// is retained in history as a "complete"/user message pair so the thread
// reads as a linear conversation like blank mode.

function deriveGithubMessages(state: OnboardingState): OnboardingMessage[] {
  const messages: OnboardingMessage[] = [];
  const setup = state.githubSetup;
  const llmCursor = state.githubLlmKeyCursor ?? 0;
  const whCursor = state.githubWarehouseCursor ?? 0;
  const isDemo = state.mode === "demo";

  messages.push({
    id: "welcome",
    role: "assistant",
    content: isDemo
      ? "Welcome to your demo workspace. The sample agents and DuckDB data are already in place — I just need an LLM API key so the agents can answer questions about the data."
      : "Welcome back. Your repository is imported — I just need a few secrets from you before you can start asking questions. Skip anything you're not ready to set up yet.",
    status: state.step === "github_loading" ? undefined : "complete"
  });

  if (state.step === "github_loading") {
    messages.push({
      id: "github_loading",
      role: "assistant",
      content: isDemo
        ? "Setting up your demo workspace…"
        : "Scanning config.yml to see what setup is needed…",
      status: "working"
    });
    return messages;
  }

  if (!setup) {
    return messages;
  }

  const totalLlmKeys = setup.missing_llm_key_vars.length;
  for (let i = 0; i < totalLlmKeys; i++) {
    // While the LLM-key flow is active, only render completed prompts and the
    // current cursor — never preview upcoming entries. Showing a bullet for
    // the next key reads as though the assistant is requesting several keys
    // at once, even though the input field only appears for the active one.
    if (state.step === "github_llm_keys" && i > llmCursor) break;
    const keyVar = setup.missing_llm_key_vars[i];
    const isActive = state.step === "github_llm_keys" && i === llmCursor;
    const isPast = i < llmCursor || state.step !== "github_llm_keys";
    const id = `github_llm_key_${keyVar.var_name}`;
    const submitting = isActive && state.githubLlmKeyTesting === true;
    const activeError = isActive ? state.githubLlmKeyError : undefined;
    messages.push({
      id,
      role: "assistant",
      content: `Enter your ${keyVar.vendor} API key (stored as \`${keyVar.var_name}\`). This will be saved as a workspace secret.`,
      inputBlock: isActive
        ? {
            type: "secure_input",
            label: keyVar.var_name,
            placeholder: `Enter your ${keyVar.vendor} API key…`,
            buttonLabel: submitting ? "Verifying key…" : "Save & Continue",
            busy: submitting,
            errorMessage: activeError
          }
        : undefined,
      status: isPast ? "complete" : activeError ? "error" : submitting ? "working" : undefined,
      allowSkip: isActive && !submitting
    });
    if (isPast) {
      messages.push({
        id: `${id}_answer`,
        role: "user",
        content: "API key saved"
      });
    }
  }

  const totalWarehouses = setup.warehouses.length;
  const warehousesActive = state.step === "github_warehouse_creds";

  if (warehousesActive || state.step === "complete") {
    for (let i = 0; i < totalWarehouses; i++) {
      // Same one-prompt-at-a-time rule as the LLM key loop above: while the
      // warehouse step is active, don't pre-render upcoming credential forms.
      if (warehousesActive && i > whCursor) break;
      const warehouse = setup.warehouses[i];
      const isActive = warehousesActive && i === whCursor;
      const isPast = i < whCursor || state.step === "complete";
      const id = `github_warehouse_${warehouse.name}`;
      const fields = warehouse.missing_vars.map((v) => ({
        key: v.var_name,
        label: humanizeWarehouseField(v.field, v.var_name),
        placeholder: v.var_name,
        type: (v.field === "port" ? "text" : "password") as "text" | "password",
        required: v.required
      }));

      const submitting = isActive && state.githubWarehouseSubmitting === true;
      const activeError = isActive ? state.githubWarehouseError : undefined;
      messages.push({
        id,
        role: "assistant",
        content: `Fill in credentials for the \`${warehouse.name}\` (${warehouse.dialect}) warehouse. Each field maps to a \`*_var\` entry in the repo's config.yml.`,
        inputBlock: isActive
          ? {
              type: "credential_form",
              fields,
              buttonLabel: submitting ? "Testing connection…" : "Save & Test Connection",
              busy: submitting,
              errorMessage: activeError
            }
          : undefined,
        status: isPast ? "complete" : undefined,
        allowSkip: isActive && !submitting
      });

      if (isPast) {
        const result = state.githubWarehouseResults?.[warehouse.name];
        const label =
          result === "success"
            ? "Connected"
            : result === "failed"
              ? "Saved (connection failed)"
              : "Skipped";
        messages.push({
          id: `${id}_answer`,
          role: "user",
          content: label
        });
      }
    }
  }

  return messages;
}

/** Pretty field label for a warehouse credential prompt. */
function humanizeWarehouseField(field: string, varName: string): string {
  const fieldLabel = field
    .split("_")
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ");
  return `${fieldLabel} (${varName})`;
}

// ── Messages derived from state ─────────────────────────────────────────────

export function deriveMessages(state: OnboardingState): OnboardingMessage[] {
  // `github` and `demo` share the same "scan config.yml, fill missing
  // secrets" thread shape — `deriveGithubMessages` already branches on
  // `state.mode === "demo"` for mode-specific copy.
  if (state.mode === "github" || state.mode === "demo") {
    return deriveGithubMessages(state);
  }

  const messages: OnboardingMessage[] = [];
  const currentIdx = stepIndex(state.step);

  messages.push({
    id: "welcome",
    role: "assistant",
    content:
      "Welcome to Oxygen. I'll help you set up your workspace — connect your LLM, link your data warehouse, and build your first semantic model.\n\nLet's get started.",
    status: currentIdx > 0 ? "complete" : undefined
  });

  if (currentIdx < stepIndex("llm_provider")) return messages;

  messages.push({
    id: "llm_provider",
    role: "assistant",
    content: "First, which LLM provider would you like to use?",
    inputBlock:
      currentIdx === stepIndex("llm_provider")
        ? {
            type: "selection_cards",
            options: [
              {
                id: "anthropic",
                label: "Anthropic",
                description: "Claude models — recommended for analytics"
              },
              { id: "openai", label: "OpenAI", description: "GPT-5.4, GPT-5.4 Mini, and GPT-4.1" }
            ]
          }
        : undefined,
    userSelection: state.llmProvider,
    status: currentIdx > stepIndex("llm_provider") ? "complete" : undefined
  });

  if (state.llmProvider && currentIdx > stepIndex("llm_provider")) {
    messages.push({
      id: "llm_provider_answer",
      role: "user",
      content: state.llmProvider.charAt(0).toUpperCase() + state.llmProvider.slice(1)
    });
  }

  if (currentIdx < stepIndex("llm_model")) return messages;

  const modelOptions = getModelOptions(state.llmProvider);
  messages.push({
    id: "llm_model",
    role: "assistant",
    content: "Which model would you like to use? You can always change this later.",
    inputBlock:
      currentIdx === stepIndex("llm_model")
        ? { type: "selection_cards", collapseAfter: 2, options: modelOptions }
        : undefined,
    userSelection: state.llmModel,
    status: currentIdx > stepIndex("llm_model") ? "complete" : undefined
  });

  if (state.llmModel && currentIdx > stepIndex("llm_model")) {
    const modelLabel =
      MODEL_OPTIONS[state.llmProvider ?? "anthropic"]?.find((m) => m.id === state.llmModel)
        ?.label ?? state.llmModel;
    messages.push({
      id: "llm_model_answer",
      role: "user",
      content: modelLabel
    });
  }

  if (currentIdx < stepIndex("llm_key")) return messages;

  // LLM API Key
  const providerName = state.llmProvider
    ? state.llmProvider.charAt(0).toUpperCase() + state.llmProvider.slice(1)
    : "your provider";

  messages.push({
    id: "llm_key",
    role: "assistant",
    content: `Enter your ${providerName} API key. This will be stored securely as a project secret.`,
    inputBlock:
      currentIdx === stepIndex("llm_key")
        ? {
            type: "secure_input",
            label: "API Key",
            placeholder: `Enter your ${providerName} API key...`,
            buttonLabel: state.llmKeyTesting ? "Verifying key…" : "Save & Continue",
            busy: state.llmKeyTesting,
            errorMessage: state.llmKeyError
          }
        : undefined,
    status:
      currentIdx > stepIndex("llm_key")
        ? "complete"
        : state.llmKeyError
          ? "error"
          : state.llmKeyTesting
            ? "working"
            : undefined
  });

  if (state.llmApiKey && currentIdx > stepIndex("llm_key")) {
    messages.push({
      id: "llm_key_answer",
      role: "user",
      content: "API key saved"
    });
    messages.push({
      id: "llm_configured",
      role: "assistant",
      content: `${providerName} API key saved. You can update this later in Settings > Secrets.`,
      status: "complete"
    });
  }

  if (currentIdx < stepIndex("warehouse_type")) return messages;

  messages.push({
    id: "warehouse_type",
    role: "assistant",
    content:
      "Now let's connect your data warehouse. You can always add more sources later in Settings.",
    inputBlock:
      currentIdx === stepIndex("warehouse_type")
        ? {
            type: "selection_cards",
            options: [
              { id: "snowflake", label: "Snowflake", description: "Cloud data platform" },
              { id: "clickhouse", label: "ClickHouse", description: "Columnar analytics database" },
              { id: "bigquery", label: "BigQuery", description: "Google Cloud data warehouse" },
              { id: "duckdb", label: "DuckDB", description: "In-process analytics database" }
            ]
          }
        : undefined,
    userSelection: state.warehouseType,
    status: currentIdx > stepIndex("warehouse_type") ? "complete" : undefined
  });

  if (state.warehouseType && currentIdx > stepIndex("warehouse_type")) {
    messages.push({
      id: "warehouse_type_answer",
      role: "user",
      content: state.warehouseType.charAt(0).toUpperCase() + state.warehouseType.slice(1)
    });
  }

  if (currentIdx < stepIndex("warehouse_credentials")) return messages;

  // Warehouse credentials
  const isDuckdb = state.warehouseType === "duckdb";
  const warehouseFields = getWarehouseFields(state.warehouseType ?? "bigquery");
  const fileUploadField = warehouseFields.find((f) => f.type === "file_upload");
  // For DuckDB the CTA also runs the upload, so name it accordingly. Other
  // warehouses still just test a remote connection.
  const credentialsButtonLabel = isDuckdb ? "Upload & Connect" : "Test Connection";
  // Pre-populate any already-uploaded paths so navigating back here doesn't
  // strand the user with a disabled CTA + a backend that refuses duplicates.
  const initialUploadedFiles =
    fileUploadField && (state.uploadedWarehouseFiles?.length ?? 0) > 0
      ? { [fileUploadField.key]: state.uploadedWarehouseFiles ?? [] }
      : undefined;
  // Hitting Back to this step clears `warehouseCredentials`, but for DuckDB
  // the `dataset` value is just the upload subdir we still have around. Carry
  // it over so a retry without re-picking files doesn't submit an empty path.
  const initialCredentialValues =
    fileUploadField && state.uploadedWarehouseSubdir
      ? {
          ...(state.warehouseCredentials ?? {}),
          [fileUploadField.key]: state.uploadedWarehouseSubdir
        }
      : state.warehouseCredentials;

  messages.push({
    id: "warehouse_credentials",
    role: "assistant",
    content: `Enter your ${state.warehouseType} connection details.`,
    inputBlock:
      currentIdx === stepIndex("warehouse_credentials")
        ? {
            type: "credential_form",
            fields: warehouseFields,
            buttonLabel: credentialsButtonLabel,
            initialValues: initialCredentialValues,
            initialUploadedFiles
          }
        : undefined,
    status: currentIdx > stepIndex("warehouse_credentials") ? "complete" : undefined
  });

  if (currentIdx < stepIndex("connection_test")) return messages;

  if (state.connectionStatus === "testing") {
    messages.push({
      id: "connection_test",
      role: "assistant",
      content: "Testing connection to your warehouse...",
      status: "working"
    });
  } else if (state.connectionStatus === "success") {
    messages.push({
      id: "connection_test",
      role: "assistant",
      content: "Connection successful. Warehouse connected.",
      status: "complete"
    });
  } else if (state.connectionStatus === "failed") {
    messages.push({
      id: "connection_test",
      role: "assistant",
      content: `Connection failed: ${state.connectionError ?? "Unknown error"}. Please check your credentials and try again.`,
      status: "error",
      inputBlock: {
        type: "credential_form",
        fields: warehouseFields,
        buttonLabel: isDuckdb ? "Upload & Retry" : "Retry Connection",
        initialValues: initialCredentialValues,
        initialUploadedFiles
      },
      goBackStep: "warehouse_type",
      goBackLabel: "Change warehouse type"
    });
  }

  if (currentIdx < stepIndex("schema_discovery")) return messages;

  if (state.schemaDiscoveryError) {
    messages.push({
      id: "schema_discovery",
      role: "assistant",
      content: `Schema inspection failed: ${state.schemaDiscoveryError}. Let's try again.`,
      status: "error",
      inputBlock: { type: "confirm_button", label: "Retry Schema Discovery" },
      goBackStep: "warehouse_credentials",
      goBackLabel: "Change connection details"
    });
  } else if (state.discoveredSchemas.length === 0 && currentIdx === stepIndex("schema_discovery")) {
    messages.push({
      id: "schema_discovery",
      role: "assistant",
      content: state.schemaDiscoveryStatus ?? "Inspecting your warehouse schema...",
      status: "working"
    });
  } else if (state.discoveredSchemas.length > 0) {
    const totalTables = state.discoveredSchemas.reduce((n, s) => n + s.tableCount, 0);
    messages.push({
      id: "schema_discovery",
      role: "assistant",
      content: `Found ${state.discoveredSchemas.length} schema${state.discoveredSchemas.length > 1 ? "s" : ""} with ${totalTables} table${totalTables > 1 ? "s" : ""}.`,
      status: "complete"
    });
  }

  if (currentIdx < stepIndex("table_selection")) return messages;

  messages.push({
    id: "table_selection",
    role: "assistant",
    content:
      "Select the tables you'd like to include in your semantic model. These will be used to build views, measures, and dimensions.",
    inputBlock:
      currentIdx === stepIndex("table_selection") ? { type: "table_selector" } : undefined,
    status: currentIdx > stepIndex("table_selection") ? "complete" : undefined
  });

  if (state.selectedTables.length > 0 && currentIdx > stepIndex("table_selection")) {
    messages.push({
      id: "table_selection_answer",
      role: "user",
      content: `Selected ${state.selectedTables.length} table${state.selectedTables.length > 1 ? "s" : ""}`
    });
  }

  if (currentIdx < stepIndex("building")) return messages;

  // Building phase — show a discrete message per build phase
  if (state.buildError) {
    messages.push({
      id: "building",
      role: "assistant",
      content: `Something went wrong: ${state.buildError}. Let's try again.`,
      status: "error",
      inputBlock: { type: "confirm_button", label: "Retry Build" }
    });
    return messages;
  }

  const ps = state.phaseStatuses ?? {};
  const vs = state.viewRunStatuses ?? {};
  const isDone = state.step === "complete";
  const db = state.warehouseType ?? "warehouse";

  // Semantic model message — shown as soon as the building step starts
  if (currentIdx >= stepIndex("building")) {
    // Semantic model is "done" when config + all views are complete
    const totalViews = state.selectedTables.length;
    const doneViews = Object.values(vs).filter((s) => s === "done" || s === "failed").length;
    const configDone = ps.config === "done";
    const allViewsDone = totalViews > 0 && doneViews === totalViews && configDone;
    const anyViewRunning = Object.values(vs).some((s) => s === "running");

    const status: OnboardingMessage["status"] =
      allViewsDone || isDone
        ? "complete"
        : ps.config === "failed"
          ? "error"
          : ps.config === "running" || anyViewRunning
            ? "working"
            : ps.semantic_layer === "done" || ps.semantic_layer === "failed"
              ? ps.semantic_layer === "done"
                ? "complete"
                : "error"
              : ps.semantic_layer === "running"
                ? "working"
                : "working";

    const viewProgress =
      totalViews > 0 && (ps.config || anyViewRunning || doneViews > 0)
        ? ` (${doneViews}/${totalViews} views)`
        : "";

    messages.push({
      id: "phase_semantic",
      role: "assistant",
      content: `Inspecting your ${db} tables and building the semantic model${viewProgress}…`,
      status
    });
  }

  // Agent message — shown once semantic model phase has started
  const semanticStarted = ps.config || ps.semantic_layer;
  if (semanticStarted) {
    const totalViews = state.selectedTables.length;
    const doneViews = Object.values(vs).filter((s) => s === "done" || s === "failed").length;
    const configDone = ps.config === "done";
    const semanticDone =
      (totalViews > 0 && doneViews === totalViews && configDone) ||
      ps.semantic_layer === "done" ||
      isDone;

    const agentStatus: OnboardingMessage["status"] =
      ps.agent === "done" || isDone
        ? "complete"
        : ps.agent === "failed"
          ? "error"
          : ps.agent === "running"
            ? "working"
            : semanticDone
              ? "working"
              : "pending";
    messages.push({
      id: "phase_agent",
      role: "assistant",
      content: semanticDone ? "Creating your analytics agent…" : "Analytics agent",
      status: agentStatus
    });
  }

  return messages;
}

// ── Build phase sub-milestones ───────────────────────────────────────────────

function toMilestoneStatus(
  ps: PhaseStatus | undefined,
  isDone: boolean
): "pending" | "active" | "complete" | "error" {
  if (isDone) return "complete";
  if (!ps) return "pending";
  if (ps === "running") return "active";
  if (ps === "done") return "complete";
  return "error";
}

function buildPhaseMilestones(
  state: OnboardingState,
  isDone: boolean
): import("./types").Milestone[] {
  const ps = state.phaseStatuses ?? {};
  const viewStatuses = state.viewRunStatuses ?? {};
  const totalViews = state.selectedTables.length;
  const doneViews = Object.values(viewStatuses).filter(
    (s) => s === "done" || s === "failed"
  ).length;

  // Semantic model milestone — no per-file detail here; the BUILDING section handles that
  const configDone = ps.config === "done";
  const allViewsDone = totalViews > 0 && doneViews === totalViews;
  const semanticStatus: "pending" | "active" | "complete" | "error" = isDone
    ? "complete"
    : allViewsDone && configDone
      ? "complete"
      : ps.config === "running" || Object.values(viewStatuses).some((s) => s === "running")
        ? "active"
        : ps.semantic_layer
          ? toMilestoneStatus(ps.semantic_layer, isDone)
          : "pending";

  const milestones: import("./types").Milestone[] = [
    {
      id: "build-semantic",
      label: "Semantic Model",
      status: semanticStatus
    },
    {
      id: "build-agent",
      label: "Analytics Agent",
      status: toMilestoneStatus(ps.agent, isDone)
    }
  ];

  return milestones;
}

// ── Right rail state derived from onboarding state ──────────────────────────

export function deriveRailState(state: OnboardingState): OnboardingRailState {
  // Demo reuses the github rail (LLM keys + warehouses milestone), since
  // both flows derive their progress from `githubSetup`.
  if (state.mode === "github" || state.mode === "demo") {
    return deriveGithubRailState(state);
  }

  const currentIdx = stepIndex(state.step);
  const milestones = [
    {
      id: "llm",
      label: "LLM Provider",
      status:
        currentIdx > stepIndex("llm_key")
          ? ("complete" as const)
          : currentIdx >= stepIndex("llm_provider")
            ? ("active" as const)
            : ("pending" as const),
      // The rail no longer has a separate "Connected" section — show the vendor
      // (Anthropic / OpenAI) inline here so the user can see which service is
      // connected at a glance.
      detail: state.llmProvider
        ? state.llmProvider.charAt(0).toUpperCase() + state.llmProvider.slice(1)
        : undefined
    },
    {
      id: "warehouse",
      label: "Data Warehouse",
      // Once the user is past the connection-test step the warehouse is
      // connected — treat it as complete regardless of the live
      // `connectionStatus` flag, which can drift (e.g. while re-running
      // discovery). "failed" is only surfaced during the active connection
      // test so the user can retry with new credentials.
      status:
        currentIdx > stepIndex("connection_test")
          ? ("complete" as const)
          : state.connectionStatus === "success"
            ? ("complete" as const)
            : state.connectionStatus === "failed"
              ? ("error" as const)
              : currentIdx >= stepIndex("warehouse_type")
                ? ("active" as const)
                : ("pending" as const),
      detail: state.warehouseType
        ? state.warehouseType.charAt(0).toUpperCase() + state.warehouseType.slice(1)
        : undefined
    },
    {
      id: "schema",
      label: "Schema Discovery",
      status:
        state.discoveredSchemas.length > 0
          ? ("complete" as const)
          : currentIdx >= stepIndex("schema_discovery")
            ? ("active" as const)
            : ("pending" as const),
      detail:
        state.discoveredSchemas.length > 0
          ? `${state.discoveredSchemas.reduce((n, s) => n + s.tableCount, 0)} tables found`
          : undefined
    },
    {
      id: "tables",
      label: "Table Selection",
      status:
        state.selectedTables.length > 0
          ? ("complete" as const)
          : currentIdx >= stepIndex("table_selection")
            ? ("active" as const)
            : ("pending" as const),
      detail:
        state.selectedTables.length > 0 ? `${state.selectedTables.length} selected` : undefined
    },
    {
      id: "build",
      label: "Build Workspace",
      status:
        state.step === "complete"
          ? ("complete" as const)
          : state.step === "building"
            ? ("active" as const)
            : ("pending" as const),
      children:
        state.step === "building" || state.step === "complete"
          ? buildPhaseMilestones(state, state.step === "complete")
          : undefined
    }
  ];

  const connectedServices = [];
  if (state.llmProvider && currentIdx > stepIndex("llm_key")) {
    connectedServices.push({
      type: "llm" as const,
      name: state.llmProvider.charAt(0).toUpperCase() + state.llmProvider.slice(1),
      status: "connected" as const
    });
  }
  if (state.connectionStatus === "success" && state.warehouseType) {
    connectedServices.push({
      type: "warehouse" as const,
      name: state.warehouseType.charAt(0).toUpperCase() + state.warehouseType.slice(1),
      status: "connected" as const
    });
  }

  // Build expected file list: config + views + agent.
  const expectedFiles: import("./types").ExpectedFile[] =
    state.selectedTables.length > 0
      ? [
          { name: "config.yml", type: "config" },
          ...state.selectedTables.map((t) => {
            const tableName = t.split(".").pop() ?? t;
            return { name: tableName, type: "view" as const };
          }),
          { name: "analytics.agentic.yml", type: "agentic" }
        ]
      : [];

  const isBuildRunning = state.step === "building";

  return {
    milestones,
    connectedServices,
    selectedTables: state.selectedTables,
    generatedArtifacts: [],
    expectedFiles,
    isBuildRunning
  };
}

// ── GitHub mode: right-rail state ──────────────────────────────────────────
//
// The rail shows two milestone rows — LLM keys + warehouses — with per-entry
// detail so the user can see at a glance what's done / remaining. There is no
// "Build workspace" milestone: github mode doesn't build anything.

function deriveGithubRailState(state: OnboardingState): OnboardingRailState {
  const setup = state.githubSetup;
  const totalLlmKeys = setup?.missing_llm_key_vars.length ?? 0;
  const llmCursor = state.githubLlmKeyCursor ?? 0;
  const totalWarehouses = setup?.warehouses.length ?? 0;
  const whCursor = state.githubWarehouseCursor ?? 0;
  const isComplete = state.step === "complete";

  const llmStatus: "pending" | "active" | "complete" | "error" =
    isComplete || (totalLlmKeys > 0 && llmCursor >= totalLlmKeys)
      ? "complete"
      : state.step === "github_llm_keys"
        ? "active"
        : totalLlmKeys === 0
          ? "complete"
          : "pending";

  const whStatus: "pending" | "active" | "complete" | "error" =
    isComplete || (totalWarehouses > 0 && whCursor >= totalWarehouses)
      ? "complete"
      : state.step === "github_warehouse_creds"
        ? "active"
        : totalWarehouses === 0
          ? "complete"
          : "pending";

  const connectedServices: OnboardingRailState["connectedServices"] = [];
  if (totalLlmKeys === 0 || llmCursor >= totalLlmKeys) {
    connectedServices.push({
      type: "llm",
      name: "LLM configured",
      status: "connected"
    });
  }
  const successfulWarehouses = Object.entries(state.githubWarehouseResults ?? {})
    .filter(([, r]) => r === "success")
    .map(([name]) => name);
  for (const name of successfulWarehouses) {
    connectedServices.push({
      type: "warehouse",
      name,
      status: "connected"
    });
  }

  return {
    milestones: [
      {
        id: "llm",
        label: "LLM API keys",
        status: llmStatus,
        detail:
          totalLlmKeys > 0
            ? `${Math.min(llmCursor, totalLlmKeys)} of ${totalLlmKeys}`
            : "none required"
      },
      {
        id: "warehouse",
        label: "Warehouse credentials",
        status: whStatus,
        detail:
          totalWarehouses > 0
            ? `${Math.min(whCursor, totalWarehouses)} of ${totalWarehouses}`
            : "none required"
      }
    ],
    connectedServices,
    selectedTables: [],
    generatedArtifacts: [],
    expectedFiles: [],
    isBuildRunning: false,
    isBuildComplete: isComplete
  };
}

// ── Model options per provider ───────────────────────────────────────────────

interface ModelOption {
  id: string; // config name written to config.yml
  label: string; // display name
  description: string;
  modelRef: string; // provider's model ID
  vendor: string; // vendor for config.yml
}

function getModelOptions(provider?: LlmProvider): SelectionOption[] {
  const models = MODEL_OPTIONS[provider ?? "anthropic"] ?? [];
  return models.map((m) => ({
    id: m.id,
    label: m.label,
    description: m.description
  }));
}

export const MODEL_OPTIONS: Record<LlmProvider, ModelOption[]> = {
  anthropic: [
    {
      id: "claude-opus-4-7",
      label: "Claude Opus 4.7",
      description: "Most capable, deepest reasoning",
      modelRef: "claude-opus-4-7",
      vendor: "anthropic"
    },
    {
      id: "claude-sonnet-4-6",
      label: "Claude Sonnet 4.6",
      description: "Balanced speed and capability",
      modelRef: "claude-sonnet-4-6",
      vendor: "anthropic"
    }
  ],
  openai: [
    {
      id: "gpt-5.4",
      label: "GPT-5.4",
      description: "Most capable, best reasoning",
      modelRef: "gpt-5.4",
      vendor: "openai"
    },
    {
      id: "gpt-5.4-mini",
      label: "GPT-5.4 Mini",
      description: "Fast and cost-effective",
      modelRef: "gpt-5.4-mini",
      vendor: "openai"
    },
    {
      id: "gpt-4.1",
      label: "GPT-4.1",
      description: "Well-tested with Oxygen",
      modelRef: "gpt-4.1",
      vendor: "openai"
    }
  ]
};

// ── Warehouse credential field definitions ──────────────────────────────────

function getWarehouseFields(type: WarehouseType) {
  switch (type) {
    case "bigquery":
      return [
        {
          key: "dataset",
          label: "Dataset",
          placeholder: "my_dataset",
          type: "text" as const,
          required: true
        },
        {
          key: "key_json",
          label: "Service Account Key (JSON)",
          placeholder: '{"type": "service_account", ...}',
          type: "textarea" as const,
          rows: 6,
          required: true,
          validateAs: "json" as const
        }
      ];
    case "snowflake":
      return [
        {
          key: "account",
          label: "Account",
          placeholder: "org-account",
          type: "text" as const,
          required: true
        },
        {
          key: "warehouse",
          label: "Warehouse",
          placeholder: "COMPUTE_WH",
          type: "text" as const,
          required: true
        },
        {
          key: "database",
          label: "Database",
          placeholder: "MY_DB",
          type: "text" as const,
          required: true
        },
        {
          key: "username",
          label: "Username",
          placeholder: "admin",
          type: "text" as const,
          required: true
        },
        {
          key: "password",
          label: "Password",
          placeholder: "Enter password",
          type: "password" as const,
          required: true
        }
      ];
    case "clickhouse":
      return [
        {
          key: "host",
          label: "Host",
          placeholder: "localhost:8123",
          type: "text" as const,
          required: true
        },
        {
          key: "database",
          label: "Database",
          placeholder: "default",
          type: "text" as const,
          required: true
        },
        {
          key: "user",
          label: "Username",
          placeholder: "default",
          type: "text" as const,
          required: true
        },
        {
          key: "password",
          label: "Password",
          placeholder: "Enter password",
          type: "password" as const
        }
      ];
    case "duckdb":
      return [
        {
          key: "dataset",
          label: "Upload data files",
          placeholder: "Drop CSV or Parquet files here, or click to browse",
          type: "file_upload" as const,
          required: true,
          accept: ".csv,.parquet",
          multiple: true,
          helperText:
            "Files are uploaded to .db/ inside your workspace. DuckDB reads each file as a table named after the filename."
        }
      ];
  }
}

// ── Hook ────────────────────────────────────────────────────────────────────

function saveState(state: OnboardingState) {
  if (!state.storageKey) return;
  try {
    const { llmApiKey: _, warehouseCredentials: __, ...safe } = state;
    // Persist only the schema skeleton — drop per-schema tables and transient
    // load flags. On warehouses with thousands of tables this keeps
    // localStorage usage small and re-renders cheap; schemas are re-hydrated
    // on expand, and the resync hydrates columns for the picked tables.
    const persistable = {
      ...safe,
      discoveredSchemas: safe.discoveredSchemas.map((schema) => ({
        schema: schema.schema,
        tables: [] as SchemaInfo["tables"],
        tableCount: schema.tableCount,
        loaded: false
      }))
    };
    localStorage.setItem(storageKey(state.storageKey), JSON.stringify(persistable));
  } catch {
    // localStorage may be unavailable
  }
}

function loadState(storageKeyId: string): OnboardingState {
  try {
    localStorage.removeItem(LEGACY_GLOBAL_KEY);
  } catch {
    // ignore
  }
  if (!storageKeyId) return initialState;
  const fresh: OnboardingState = { ...initialState, storageKey: storageKeyId };
  try {
    const raw = localStorage.getItem(storageKey(storageKeyId));
    if (!raw) return fresh;
    const parsed = JSON.parse(raw) as PersistableState;
    if (parsed?.storageKey !== storageKeyId) return fresh;
    if (typeof parsed?.step !== "string" || !VALID_STEPS.has(parsed.step)) {
      return fresh;
    }
    // "building" without phaseRunIds means runs were lost — restart from selection
    if (parsed.step === "building" && !parsed.phaseRunIds) {
      return { ...initialState, ...parsed, step: "table_selection" };
    }
    // In-flight test submissions don't survive a reload — clear them so the
    // form isn't permanently stuck in a disabled "Testing…" state.
    return {
      ...initialState,
      ...parsed,
      githubWarehouseSubmitting: false,
      githubLlmKeyTesting: false,
      llmKeyTesting: false
    };
  } catch {
    return fresh;
  }
}

export function useOnboardingOrchestrator(storageKeyId: string) {
  const [state, dispatch] = useReducer(reducer, storageKeyId, loadState);

  // Persist to localStorage on state changes. Debounced because dispatches
  // can burst during lazy table loads / per-schema expansion, and JSON
  // serialization of the full state is not free on large warehouses.
  useEffect(() => {
    const handle = setTimeout(() => saveState(state), 150);
    return () => clearTimeout(handle);
  }, [state]);

  const messages = useMemo(() => deriveMessages(state), [state]);
  const railState = useMemo(() => deriveRailState(state), [state]);

  const setLlmProvider = useCallback(
    (provider: LlmProvider) => dispatch({ type: "SET_LLM_PROVIDER", provider }),
    []
  );

  const setLlmModel = useCallback(
    (model: string, modelRef: string, vendor: string) =>
      dispatch({ type: "SET_LLM_MODEL", model, modelRef, vendor }),
    []
  );

  const setLlmKey = useCallback((apiKey: string) => dispatch({ type: "SET_LLM_KEY", apiKey }), []);

  const startLlmKeyTest = useCallback(() => dispatch({ type: "START_LLM_KEY_TEST" }), []);

  const failLlmKeyTest = useCallback(
    (error: string) => dispatch({ type: "FAIL_LLM_KEY_TEST", error }),
    []
  );

  const setWarehouseType = useCallback(
    (warehouseType: WarehouseType) => dispatch({ type: "SET_WAREHOUSE_TYPE", warehouseType }),
    []
  );

  const setWarehouseCredentials = useCallback(
    (credentials: Record<string, string>) =>
      dispatch({ type: "SET_WAREHOUSE_CREDENTIALS", credentials }),
    []
  );

  const setUploadedWarehouseFiles = useCallback(
    (files: string[], subdir: string) =>
      dispatch({ type: "SET_UPLOADED_WAREHOUSE_FILES", files, subdir }),
    []
  );

  const setConnectionStatus = useCallback(
    (status: ConnectionStatus, error?: string) =>
      dispatch({ type: "SET_CONNECTION_STATUS", status, error }),
    []
  );

  const setDiscoveredSchemas = useCallback(
    (schemas: SchemaInfo[]) => dispatch({ type: "SET_DISCOVERED_SCHEMAS", schemas }),
    []
  );

  const hydrateDiscoveredSchemas = useCallback(
    (schemas: SchemaInfo[]) => dispatch({ type: "HYDRATE_DISCOVERED_SCHEMAS", schemas }),
    []
  );

  const setSchemaTablesStatus = useCallback(
    (schema: string, status: "loading" | "error", error?: string) =>
      dispatch({ type: "SET_SCHEMA_TABLES_STATUS", schema, status, error }),
    []
  );

  const setSchemaTables = useCallback(
    (schema: string, tables: SchemaInfo["tables"]) =>
      dispatch({ type: "SET_SCHEMA_TABLES", schema, tables }),
    []
  );

  const setSchemaDiscoveryError = useCallback(
    (error: string) => dispatch({ type: "SET_SCHEMA_DISCOVERY_ERROR", error }),
    []
  );

  const setSchemaDiscoveryStatus = useCallback(
    (message: string | undefined) => dispatch({ type: "SET_SCHEMA_DISCOVERY_STATUS", message }),
    []
  );

  const setSelectedTables = useCallback(
    (tables: string[]) => dispatch({ type: "SET_SELECTED_TABLES", tables }),
    []
  );

  const startPhase = useCallback(
    (phase: BuildPhase, threadId: string, runId: string) =>
      dispatch({ type: "START_PHASE", phase, threadId, runId }),
    []
  );

  const setPhaseStatus = useCallback(
    (phase: BuildPhase, status: PhaseStatus) =>
      dispatch({ type: "SET_PHASE_STATUS", phase, status }),
    []
  );

  const startViewRun = useCallback(
    (table: string, runId: string) => dispatch({ type: "START_VIEW_RUN", table, runId }),
    []
  );

  const setViewRunStatus = useCallback(
    (table: string, status: PhaseStatus) =>
      dispatch({ type: "SET_VIEW_RUN_STATUS", table, status }),
    []
  );

  const setBuildError = useCallback(
    (error: string) => dispatch({ type: "SET_BUILD_ERROR", error }),
    []
  );

  const stopBuild = useCallback((message: string) => dispatch({ type: "STOP_BUILD", message }), []);

  const complete = useCallback(
    (createdFiles: string[], sampleQuestions: string[]) =>
      dispatch({ type: "COMPLETE", createdFiles, sampleQuestions }),
    []
  );

  const goToStep = useCallback(
    (step: OnboardingStep) => dispatch({ type: "GO_TO_STEP", step }),
    []
  );

  const setGithubSetup = useCallback(
    (setup: GithubSetup) => dispatch({ type: "SET_GITHUB_SETUP", setup }),
    []
  );

  const advanceGithubLlmKey = useCallback(() => dispatch({ type: "ADVANCE_GITHUB_LLM_KEY" }), []);

  const startGithubLlmKeyTest = useCallback(
    () => dispatch({ type: "START_GITHUB_LLM_KEY_TEST" }),
    []
  );

  const failGithubLlmKeyTest = useCallback(
    (error: string) => dispatch({ type: "FAIL_GITHUB_LLM_KEY_TEST", error }),
    []
  );

  const advanceGithubWarehouse = useCallback(
    (warehouseName: string, result: "success" | "skipped" | "failed") =>
      dispatch({ type: "ADVANCE_GITHUB_WAREHOUSE", warehouseName, result }),
    []
  );

  const startGithubWarehouseTest = useCallback(
    () => dispatch({ type: "START_GITHUB_WAREHOUSE_TEST" }),
    []
  );

  const failGithubWarehouseTest = useCallback(
    (error: string) => dispatch({ type: "FAIL_GITHUB_WAREHOUSE_TEST", error }),
    []
  );

  const initGithubFlow = useCallback(
    (mode: OnboardingMode) => dispatch({ type: "INIT_GITHUB_FLOW", mode }),
    []
  );

  return {
    state,
    messages,
    railState,
    // Actions
    setLlmProvider,
    setLlmModel,
    setLlmKey,
    startLlmKeyTest,
    failLlmKeyTest,
    setWarehouseType,
    setWarehouseCredentials,
    setUploadedWarehouseFiles,
    setConnectionStatus,
    setDiscoveredSchemas,
    hydrateDiscoveredSchemas,
    setSchemaTablesStatus,
    setSchemaTables,
    setSchemaDiscoveryError,
    setSchemaDiscoveryStatus,
    setSelectedTables,
    startPhase,
    setPhaseStatus,
    startViewRun,
    setViewRunStatus,
    setBuildError,
    stopBuild,
    complete,
    goToStep,
    setGithubSetup,
    advanceGithubLlmKey,
    startGithubLlmKeyTest,
    failGithubLlmKeyTest,
    advanceGithubWarehouse,
    startGithubWarehouseTest,
    failGithubWarehouseTest,
    initGithubFlow
  };
}
