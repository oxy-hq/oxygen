// ── Onboarding State Machine Types ──────────────────────────────────────────

export type OnboardingStep =
  | "welcome"
  | "llm_provider"
  | "llm_model"
  | "llm_key"
  | "warehouse_type"
  | "warehouse_credentials"
  | "connection_test"
  | "schema_discovery"
  | "table_selection"
  | "building" // Phase 2: hands off to real builder agent
  // GitHub-import flow: we already have the repo's config.yml, so the user
  // only needs to fill in the secrets referenced there (not pick a
  // provider / warehouse type). Each of these steps runs once per missing
  // entry — state tracks a cursor into the list.
  | "github_loading"
  | "github_llm_keys"
  | "github_warehouse_creds"
  | "github_connection_test"
  | "complete";

/**
 * Onboarding flow variant.
 *
 * - `"new"`: blank workspace — full provider / model / table selection /
 *   agentic build.
 * - `"github"`: cloned Oxy repo with existing agents + apps + semantic model —
 *   collects only the secrets the repo's `config.yml` references and validates
 *   warehouse connectivity.
 * - `"demo"`: pre-seeded sample workspace — same shape as `"github"` (everything
 *   is already built, we just need an LLM key), but always uses DuckDB so
 *   warehouse credentials don't need to be collected.
 */
export type OnboardingMode = "new" | "github" | "demo";

/** Description of a single LLM API key the repo's config.yml references. */
interface GithubSetupKeyVar {
  var_name: string;
  vendor: string;
  sample_model_name?: string;
}

/** A `*_var` secret on a warehouse that hasn't been provided yet. */
interface GithubSetupMissingVar {
  field: string;
  var_name: string;
  /** True when there's no inline plaintext in config.yml — user must fill in. */
  required: boolean;
}

/** A warehouse declared in config.yml with missing credential secrets. */
export interface GithubSetupWarehouse {
  name: string;
  dialect: string;
  missing_vars: GithubSetupMissingVar[];
}

/**
 * Result of inspecting the imported repo's `config.yml` for secrets that
 * still need to be provided before the user can query the workspace.
 */
export interface GithubSetup {
  missing_llm_key_vars: GithubSetupKeyVar[];
  warehouses: GithubSetupWarehouse[];
}

export type LlmProvider = "anthropic" | "openai";
export type WarehouseType = "bigquery" | "snowflake" | "clickhouse" | "duckdb";

export type ConnectionStatus = "idle" | "testing" | "success" | "failed";

export type BuildPhase = "semantic_layer" | "config" | "agent";
export type PhaseStatus = "running" | "done" | "failed";

export interface SchemaInfo {
  schema: string;
  /**
   * Tables discovered so far. Empty until the user expands the schema in the
   * picker or until the filtered re-sync hydrates them for the selected
   * tables. Use `tableCount` instead of `tables.length` for the total.
   */
  tables: TableInfo[];
  /** Total tables in this schema reported by the warehouse at discovery. */
  tableCount: number;
  /** Has the per-schema `inspectSchemaTables` call populated `tables`? */
  loaded: boolean;
  /** True while the per-schema fetch is in flight. */
  loading?: boolean;
  /** Error message from the most recent per-schema fetch. */
  loadError?: string;
}

interface TableInfo {
  name: string;
  /** Per-column metadata, hydrated only after a filtered re-sync. */
  columns: ColumnInfo[];
  /** Total column count reported at discovery, shown in the picker badge. */
  columnCount?: number;
  rowCount?: number;
}

interface ColumnInfo {
  name: string;
  type: string;
}

export interface OnboardingState {
  step: OnboardingStep;
  /** Matches `Workspace.storage_key`. Used as both the localStorage namespace
   * and a hydration guard — a persisted blob whose `storageKey` doesn't match
   * the active workspace is discarded rather than re-hydrating into it. */
  storageKey?: string;
  /** Which onboarding variant this run is. Defaults to `"new"`. */
  mode?: OnboardingMode;
  /** For `mode === "github"`: the setup work detected from the cloned repo. */
  githubSetup?: GithubSetup;
  /** For `mode === "github"`: cursor into `githubSetup.missing_llm_key_vars`. */
  githubLlmKeyCursor?: number;
  /**
   * For `mode === "github"`: true while the active LLM key is being verified
   * against the provider. Disables the input + flips the button label so the
   * user can't double-submit while a probe is in flight.
   */
  githubLlmKeyTesting?: boolean;
  /**
   * For `mode === "github"`: error from the most recent LLM key validation
   * attempt for the active cursor entry. Cleared on advance / re-submit.
   */
  githubLlmKeyError?: string;
  /** For `mode === "github"`: cursor into `githubSetup.warehouses`. */
  githubWarehouseCursor?: number;
  /**
   * For `mode === "github"`: per-warehouse connection-test results. Populated
   * as each warehouse test completes; used so the completion screen can show
   * which warehouses the user skipped.
   */
  githubWarehouseResults?: Record<string, "success" | "skipped" | "failed">;
  /**
   * For `mode === "github"`: true while the active warehouse's Save & Test
   * submission is in flight (saving secrets + exercising the connection).
   * Used to disable the form and surface a "Testing connection…" label.
   */
  githubWarehouseSubmitting?: boolean;
  /**
   * For `mode === "github"`: connection-test error for the active warehouse.
   * Shown inline under the form so the user can correct typos and retry;
   * cleared when they resubmit or advance past this warehouse.
   */
  githubWarehouseError?: string;
  llmProvider?: LlmProvider;
  llmApiKey?: string;
  /**
   * True while the API key submitted on the `llm_key` step is being verified
   * against the provider. The submit button stays disabled and the spinner
   * label is shown until verification settles.
   */
  llmKeyTesting?: boolean;
  /**
   * Error from the most recent LLM key validation attempt. Cleared when the
   * user re-submits or navigates away from the step. Surfacing the failure
   * inline keeps the user on the same step instead of advancing into a
   * doomed warehouse-setup flow.
   */
  llmKeyError?: string;
  llmModel?: string; // config name (e.g., "claude-sonnet-4-6")
  llmModelRef?: string; // provider model ID (e.g., "claude-sonnet-4-6")
  llmVendor?: string; // vendor name for config.yml (e.g., "anthropic")
  warehouseType?: WarehouseType;
  warehouseCredentials?: Record<string, string>;
  /**
   * For the DuckDB "upload files" path: relative paths (inside workspace root)
   * of CSV / Parquet files that onboarding uploaded for the user. Empty / unset
   * means the user chose the classic "point at an existing directory" fallback.
   */
  uploadedWarehouseFiles?: string[];
  /**
   * Subdir (relative to workspace root) the uploaded files were written to.
   * Set in tandem with `uploadedWarehouseFiles`; used as `file_search_path` in
   * the generated DuckDB warehouse config and as the directory to recursively
   * delete on "Start over".
   */
  uploadedWarehouseSubdir?: string;
  connectionStatus: ConnectionStatus;
  connectionError?: string;
  discoveredSchemas: SchemaInfo[];
  schemaDiscoveryError?: string;
  /**
   * Live status text streamed from the schema-inspect SSE while the discovery
   * step is in flight. Cleared once `discoveredSchemas` is populated. Used to
   * keep the user informed during long Snowflake / BigQuery inspections.
   */
  schemaDiscoveryStatus?: string;
  selectedTables: string[]; // "schema.table" format
  /** Thread ID shared across all build phases (created on first phase). */
  builderThreadId?: string;
  /** Per-phase run IDs, set when each phase starts. */
  phaseRunIds?: Partial<Record<BuildPhase, string>>;
  /** Per-phase status, updated as runs complete. */
  phaseStatuses?: Partial<Record<BuildPhase, PhaseStatus>>;
  /** Per-table view run IDs (parallel semantic view creation). */
  viewRunIds?: Record<string, string>;
  /** Per-table view run statuses. */
  viewRunStatuses?: Record<string, PhaseStatus>;
  buildError?: string;
  /** File paths created during the build (persisted for reload). */
  createdFiles?: string[];
  /** Sample questions extracted from the app builder output (persisted for reload). */
  sampleQuestions?: string[];
}

// ── Message Model ───────────────────────────────────────────────────────────

export type OnboardingInputBlock =
  | { type: "selection_cards"; options: SelectionOption[]; collapseAfter?: number }
  | {
      type: "secure_input";
      label: string;
      placeholder: string;
      buttonLabel?: string;
      /** Disables the input + submit while a validation call is in flight. */
      busy?: boolean;
      /** Inline error surfaced under the input (e.g. invalid API key). */
      errorMessage?: string;
    }
  | {
      type: "credential_form";
      fields: CredentialField[];
      buttonLabel?: string;
      initialValues?: Record<string, string>;
      /**
       * Pre-populated uploaded paths per `file_upload` field key. Used when the
       * user navigates back to a credentials step after a previous upload so
       * the form remembers what was already sent and the CTA is not blocked by
       * an empty file list. The backend rejects re-uploading the same name, so
       * these surface as a confirmed list rather than re-prompting for picks.
       */
      initialUploadedFiles?: Record<string, string[]>;
      /** Disables the form while a submission is in flight. */
      busy?: boolean;
      /** Inline error surfaced under the form (e.g. failed connection test). */
      errorMessage?: string;
    }
  | { type: "table_selector" }
  | { type: "confirm_button"; label: string }
  | { type: "none" }; // No input needed (e.g., working/status messages)

export interface SelectionOption {
  id: string;
  label: string;
  description: string;
  icon?: string;
}

export interface CredentialField {
  key: string;
  label: string;
  placeholder: string;
  type: "text" | "password" | "number" | "textarea" | "file_upload";
  required?: boolean;
  defaultValue?: string;
  /** For `file_upload`: comma-separated accept list (e.g. ".csv,.parquet"). */
  accept?: string;
  /** For `file_upload`: whether multiple files may be selected. */
  multiple?: boolean;
  /** For `file_upload`: secondary descriptive text shown under the field. */
  helperText?: string;
  /** For `textarea`: number of visible rows. Defaults to 4. */
  rows?: number;
  /**
   * Optional client-side validation. `"json"` parses the value as JSON and
   * surfaces an inline error when malformed; an empty value defers to
   * `required`. Failed validation also blocks the submit button.
   */
  validateAs?: "json";
}

export interface OnboardingMessage {
  id: string;
  role: "assistant" | "user";
  content: string;
  inputBlock?: OnboardingInputBlock;
  status?: "working" | "complete" | "error" | "pending";
  userSelection?: string; // What the user chose (for display after answering)
  /** If set, shows a "go back" link that navigates to this step. */
  goBackStep?: OnboardingStep;
  goBackLabel?: string;
  /**
   * When true, renders a "Skip for now" link next to the input block. Used
   * in GitHub mode where any individual prompt can be skipped — the message
   * id tells the thread which advance action to dispatch.
   */
  allowSkip?: boolean;
}

// ── Right Rail Model ────────────────────────────────────────────────────────

export interface Milestone {
  id: string;
  label: string;
  status: "pending" | "active" | "complete" | "error";
  detail?: string;
  children?: Milestone[];
}

interface ConnectedService {
  type: "llm" | "warehouse";
  name: string;
  status: "connected" | "pending";
}

export interface GeneratedArtifact {
  filePath: string;
  description: string;
  type: "view" | "topic" | "app" | "agent" | "agentic" | "config";
}

export interface ExpectedFile {
  name: string;
  type: GeneratedArtifact["type"];
}

export interface OnboardingRailState {
  milestones: Milestone[];
  connectedServices: ConnectedService[];
  selectedTables: string[];
  generatedArtifacts: GeneratedArtifact[];
  /** Ordered list of expected files to create (config + views + agent). */
  expectedFiles: ExpectedFile[];
  /** Whether any build phase is currently running. */
  isBuildRunning: boolean;
  /** True when the build has finished (step === "complete"). */
  isBuildComplete?: boolean;
}

// ── Per-phase timing / progress ─────────────────────────────────────────────

/** Sub-phases shown in the right rail build progress. */
export type SubPhaseKey = "semantic" | "agent";

/** Wall-clock start/end timestamps for a sub-phase. */
interface PhaseTiming {
  start: number;
  end?: number;
}

export type PhaseTimings = Partial<Record<SubPhaseKey, PhaseTiming>>;

/** Progress snapshot for a single sub-phase (used for thread progress bars). */
export interface PhaseProgress {
  /** 0..1 progress ratio. */
  ratio: number;
  /** Pre-computed elapsed seconds (ticks with wall clock for running phases). */
  elapsedSeconds: number;
  /** Estimated total duration for this phase, seconds. */
  estimatedSeconds: number;
  /** True when the phase is still in flight. */
  isRunning: boolean;
}
