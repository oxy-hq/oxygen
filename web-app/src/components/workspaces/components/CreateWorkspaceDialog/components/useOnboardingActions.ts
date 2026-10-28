import { useCallback, useRef } from "react";
import { toast } from "sonner";
import useBuilderAvailable from "@/hooks/api/useBuilderAvailable";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { AnalyticsService } from "@/services/api/analytics";
import { DatabaseService } from "@/services/api/database";
import { OnboardingService } from "@/services/api/onboarding";
import { ThreadService } from "@/services/api/threads";
import { SecretService } from "@/services/secretService";
import type { WarehouseConfig } from "@/types/database";
import type { useOnboardingOrchestrator } from "./orchestrator";
import type {
  BuildPhase,
  GithubSetupWarehouse,
  LlmProvider,
  SchemaInfo,
  WarehouseType
} from "./types";

type Orchestrator = ReturnType<typeof useOnboardingOrchestrator>;

// ── LLM provider → env var name mapping ─────────────────────────────────────

export const LLM_KEY_VAR: Record<LlmProvider, string> = {
  anthropic: "ANTHROPIC_API_KEY",
  openai: "OPENAI_API_KEY"
};

/**
 * Map a backend `vendor` label (e.g. "Anthropic", "OpenAI", "Google") to one
 * of the providers the test-llm-key endpoint accepts. Returns `null` for
 * vendors we can't validate against (Google, Ollama) so the caller can fall
 * back to saving without verification — better to skip the check than block
 * onboarding for a vendor we don't know how to probe.
 */
function vendorToProbeProvider(vendor: string): LlmProvider | null {
  const normalised = vendor.trim().toLowerCase();
  if (normalised === "anthropic") return "anthropic";
  if (normalised === "openai") return "openai";
  return null;
}

/**
 * Translate a thrown error from the test-llm-key request into a user-facing
 * message. Network timeouts and generic Axios errors otherwise leak through
 * as "Request failed with status code 500" — useless to the user.
 */
function formatLlmKeyValidationError(err: unknown): string {
  if (err instanceof Error && err.message.toLowerCase().includes("timeout")) {
    return "The validation request timed out. Please check your network and try again.";
  }
  return "We couldn't reach the provider to verify your API key. Please try again.";
}

// ── Build WarehouseConfig from user input ───────────────────────────────────

/** Build config with raw passwords — used for testDatabaseConnection */
function buildWarehouseConfig(
  type: WarehouseType,
  credentials: Record<string, string>
): WarehouseConfig {
  switch (type) {
    case "bigquery":
      return {
        type: "bigquery",
        name: "bigquery",
        config: {
          key: credentials.key_json,
          dataset: credentials.dataset
        }
      };
    case "snowflake":
      return {
        type: "snowflake",
        name: "snowflake",
        config: {
          account: credentials.account,
          warehouse: credentials.warehouse,
          database: credentials.database,
          username: credentials.username,
          password: credentials.password
        }
      };
    case "clickhouse":
      return {
        type: "clickhouse",
        name: "clickhouse",
        config: {
          host: credentials.host,
          user: credentials.user,
          password: credentials.password,
          database: credentials.database
        }
      };
    case "duckdb":
      return {
        type: "duckdb",
        name: "duckdb",
        config: {
          file_search_path: credentials.dataset
        }
      };
  }
}

// ── Hook: wires orchestrator actions to real API calls ──────────────────────

export function useOnboardingActions(orchestrator: Orchestrator) {
  const { project, branchName } = useCurrentProjectBranch();
  const projectId = project?.id ?? "";
  const { builderModel } = useBuilderAvailable();

  // Keep a ref to the latest orchestrator state so callbacks can read current
  // values without invalidating their useCallback identity on every render.
  // Without this, any callback that lists `orchestrator` in its deps
  // re-creates on every state dispatch — useEffects that depend on it will
  // re-fire in a loop and can cause cascading API calls.
  const stateRef = useRef(orchestrator.state);
  stateRef.current = orchestrator.state;

  // Synchronous column cache keyed by "schema.table". Populated by
  // `resyncWithSelectedTables` and consumed by `startViewRun`. We can't rely
  // on `orchestrator.state.discoveredSchemas` here: hydration happens via
  // dispatch at the end of the resync, and `startViewRun` is called from the
  // same microtask that awaited the resync — React hasn't re-rendered yet, so
  // `orchestrator.state` is still the pre-hydration value with empty columns.
  // The ref bypasses that and gives the builder the real column names,
  // preventing the agent from hallucinating (e.g. inventing a `REPORT_DATE`
  // dimension when the actual column has a different name).
  const hydratedColumnsRef = useRef<Map<string, Array<{ name: string; type: string }>>>(new Map());

  // Validate the LLM API key with the provider, then save it as a project
  // secret and advance. Validation up-front prevents the user from completing
  // warehouse + table setup with a typo'd key only to hit an auth_error deep
  // in the agentic build phase. On validation failure we keep the user on
  // the same step with an inline error so they can retry.
  // Uses create-or-update to ensure the latest key value is always persisted,
  // even if a previous onboarding attempt left a stale secret behind.
  const saveLlmKey = useCallback(
    async (apiKey: string) => {
      const provider = orchestrator.state.llmProvider;
      if (!provider) return;

      orchestrator.startLlmKeyTest();

      try {
        const result = await OnboardingService.testLlmKey(projectId, provider, apiKey);
        if (!result.success) {
          orchestrator.failLlmKeyTest(
            result.message ??
              "We couldn't verify that API key with the provider. Please double-check the value."
          );
          return;
        }
      } catch (err) {
        console.warn("[onboarding] LLM key validation request failed", err);
        orchestrator.failLlmKeyTest(formatLlmKeyValidationError(err));
        return;
      }

      const keyVar = LLM_KEY_VAR[provider];
      try {
        await SecretService.createSecret(projectId, {
          name: keyVar,
          value: apiKey,
          description: `${provider} API key (created during onboarding)`
        });
      } catch (createErr) {
        // Secret likely already exists (409) — update it with the new value
        try {
          const { secrets } = await SecretService.listSecrets(projectId);
          const existing = secrets.find((s) => s.name === keyVar);
          if (existing) {
            await SecretService.updateSecret(projectId, existing.id, { value: apiKey });
          } else {
            // No existing secret found and create failed for a reason other than conflict
            console.warn(
              `[onboarding] Failed to save ${keyVar} — builder may fail with an auth error`,
              createErr
            );
            toast.error(`Failed to save ${keyVar}. Builder may fail with an auth error.`);
          }
        } catch (updateErr) {
          console.warn(
            `[onboarding] Failed to save ${keyVar} — builder may fail with an auth error`,
            updateErr
          );
          toast.error(`Failed to save ${keyVar}. Builder may fail with an auth error.`);
        }
      }

      orchestrator.setLlmKey(apiKey);
    },
    [projectId, orchestrator]
  );

  // Test warehouse connection, then create config if successful
  const testAndSaveWarehouse = useCallback(
    async (credentials: Record<string, string>) => {
      const type = orchestrator.state.warehouseType;
      if (!type) return;

      orchestrator.setWarehouseCredentials(credentials);

      const warehouseConfig = buildWarehouseConfig(type, credentials);

      // Helper: save database config with raw password.
      // The backend's createDatabaseConfig will:
      // 1. Extract the password, create a secret (e.g., CLICKHOUSE_PASSWORD)
      // 2. Replace password with password_var in config.yml
      // 3. Set the env var in the current process via env::set_var
      // This ensures the builder pipeline (which reads env vars) can find it.
      // NOTE: Do NOT pre-create secrets — that causes duplicate 500s.
      const persistConfig = async (): Promise<boolean> => {
        try {
          await DatabaseService.createDatabaseConfig(projectId, branchName, {
            warehouses: [warehouseConfig] // raw password, NOT password_var
          });
          return true;
        } catch (err) {
          // 409 = database already exists in config.yml — fine for re-runs.
          // 500 = secret conflict or other backend issue — config may still be usable.
          const status =
            err && typeof err === "object" && "response" in err
              ? (err as { response?: { status?: number } }).response?.status
              : undefined;
          if (status === 409) {
            console.info("[onboarding] Database already configured in config.yml, proceeding.");
          } else {
            console.warn("[onboarding] Failed to save database config:", err);
          }
          return false;
        }
      };

      // Track whether the connection test reported success via SSE event
      let connectionSucceeded = false;
      let connectionError: string | undefined;

      try {
        await DatabaseService.testDatabaseConnection(
          projectId,
          branchName,
          { warehouse: warehouseConfig },
          (event) => {
            if (event.type === "complete") {
              if (event.result.success) {
                connectionSucceeded = true;
              } else {
                connectionError = event.result.error_details ?? event.result.message;
              }
            }
          }
        );
      } catch {
        // SSE stream threw (common — backend errors during cleanup after a successful test).
        // connectionSucceeded may already be true from the event callback.
      }

      if (connectionSucceeded) {
        await persistConfig();
        orchestrator.setConnectionStatus("success");
        return;
      }

      if (connectionError) {
        orchestrator.setConnectionStatus("failed", connectionError);
        return;
      }

      // SSE failed without reporting success/failure — check if the database
      // is already configured (from a previous onboarding run)
      await persistConfig();
      try {
        const databases = await DatabaseService.listDatabases(projectId, branchName);
        const alreadyExists = databases.some(
          (db) => db.name === warehouseConfig.name || db.dialect === type
        );
        if (alreadyExists) {
          orchestrator.setConnectionStatus("success");
        } else {
          orchestrator.setConnectionStatus(
            "failed",
            "Connection test failed. Please check your credentials."
          );
        }
      } catch {
        orchestrator.setConnectionStatus("failed", "Could not verify database configuration.");
      }
    },
    [projectId, branchName, orchestrator]
  );

  // Schema-first discovery: one cheap `INFORMATION_SCHEMA.TABLES` scan returns
  // just `{ schema, table_count }` per schema. Tables for each schema are
  // loaded lazily by `fetchSchemaTables` when the user expands a schema in the
  // picker. This avoids the multi-minute INFORMATION_SCHEMA.COLUMNS scan the
  // previous all-at-once discovery triggered on warehouses with thousands of
  // tables.
  //
  // Retries once after a short delay if the first attempt errors out —
  // config/secrets may need time to propagate after the connection test.
  //
  // Deps are deliberately primitive-only + stable setters so the callback
  // identity doesn't change on every render. The `inFlight` ref prevents
  // re-entry if an upstream effect fires multiple times.
  const { setDiscoveredSchemas, setSchemaDiscoveryError, setSchemaDiscoveryStatus } = orchestrator;
  const discoveryInFlight = useRef(false);
  const discoverSchemas = useCallback(async () => {
    if (discoveryInFlight.current) return;
    discoveryInFlight.current = true;
    const dbName = stateRef.current.warehouseType ?? undefined;
    setSchemaDiscoveryStatus(
      dbName ? `Discovering schemas for ${dbName}...` : "Discovering schemas..."
    );

    const doDiscovery = async (): Promise<{
      schemas: Array<{ schema: string; table_count: number }> | null;
      error: string | null;
    }> => {
      try {
        const result = await DatabaseService.inspectSchemas(projectId, branchName, dbName);
        return { schemas: result.schemas, error: null };
      } catch (err) {
        console.error("Schema discovery attempt failed:", err);
        const message = err instanceof Error ? err.message : "Schema discovery request failed";
        return { schemas: null, error: message };
      }
    };

    try {
      const first = await doDiscovery();
      if (first.schemas && first.schemas.length > 0) {
        setDiscoveredSchemas(
          first.schemas.map((s) => ({
            schema: s.schema,
            tables: [],
            tableCount: s.table_count,
            loaded: false
          }))
        );
        return;
      }

      await new Promise((resolve) => setTimeout(resolve, 3000));
      const retry = await doDiscovery();
      if (retry.schemas && retry.schemas.length > 0) {
        setDiscoveredSchemas(
          retry.schemas.map((s) => ({
            schema: s.schema,
            tables: [],
            tableCount: s.table_count,
            loaded: false
          }))
        );
        return;
      }

      setSchemaDiscoveryError(
        retry.error ??
          first.error ??
          "No tables found. The database may still be syncing, or no tables are accessible."
      );
    } finally {
      discoveryInFlight.current = false;
    }
  }, [
    projectId,
    branchName,
    setDiscoveredSchemas,
    setSchemaDiscoveryError,
    setSchemaDiscoveryStatus
  ]);

  // Lazily load tables for a single schema when the user expands it. Stored
  // on the orchestrator so the tables survive collapse/expand cycles without
  // re-fetching.
  const { setSchemaTables, setSchemaTablesStatus } = orchestrator;
  const fetchSchemaTables = useCallback(
    async (schemaName: string) => {
      const existing = stateRef.current.discoveredSchemas.find((s) => s.schema === schemaName);
      if (!existing || existing.loaded || existing.loading) return;

      const dbName = stateRef.current.warehouseType ?? undefined;
      setSchemaTablesStatus(schemaName, "loading");
      try {
        const result = await DatabaseService.inspectSchemaTables(
          projectId,
          branchName,
          schemaName,
          dbName
        );
        setSchemaTables(
          schemaName,
          result.tables.map((t) => ({
            name: t.name,
            columns: [],
            columnCount: t.column_count,
            rowCount: undefined
          }))
        );
      } catch (err) {
        console.error("Failed to load tables for schema", schemaName, err);
        setSchemaTablesStatus(
          schemaName,
          "error",
          err instanceof Error ? err.message : "Failed to load tables"
        );
      }
    },
    [projectId, branchName, setSchemaTables, setSchemaTablesStatus]
  );

  // Ensure a shared builder thread exists, creating one if needed.
  const ensureThread = useCallback(
    async (label: string): Promise<string> => {
      const existing = orchestrator.state.builderThreadId;
      if (existing) return existing;
      const thread = await ThreadService.createThread(projectId, {
        title: "Onboarding: Build workspace",
        input: label,
        source: "__onboarding__",
        source_type: "analytics"
      });
      return thread.id;
    },
    [projectId, orchestrator.state.builderThreadId]
  );

  // Re-sync with only selected tables so .databases/<warehouse>/ is scoped to
  // the user's selection. After the sync completes we refresh
  // `discoveredSchemas` from the cached semantic models so the per-table column
  // metadata is available to `startViewRun` (it uses pre-fetched columns to
  // skip a DESCRIBE round-trip in the builder).
  const resyncWithSelectedTables = useCallback(
    async (tables: string[]) => {
      const dbName = stateRef.current.warehouseType ?? undefined;
      // Clear any columns cached from a previous run — keeps the ref scoped
      // to the current resync and prevents stale entries from leaking across
      // a "Start over" into a different warehouse.
      hydratedColumnsRef.current.clear();
      await DatabaseService.syncDatabase(projectId, branchName, dbName, { tables });

      try {
        const databases = await DatabaseService.listDatabases(projectId, branchName);
        // Merge hydrated column metadata back into existing discovered
        // schemas so we preserve per-schema `tableCount` + `loaded` state.
        const hydratedBySchema = new Map<string, Map<string, SchemaInfo["tables"][number]>>();
        for (const db of databases) {
          for (const [schemaName, tableMap] of Object.entries(db.datasets ?? {})) {
            let tablesForSchema = hydratedBySchema.get(schemaName);
            if (!tablesForSchema) {
              tablesForSchema = new Map();
              hydratedBySchema.set(schemaName, tablesForSchema);
            }
            for (const [tableName, model] of Object.entries(tableMap)) {
              const columns = [
                ...(model.dimensions ?? []).map((d) => ({
                  name: d.name,
                  type: d.type ?? "unknown"
                })),
                ...(model.measures ?? []).map((m) => ({
                  name: m.name,
                  type: "measure"
                }))
              ];
              tablesForSchema.set(tableName, {
                name: tableName,
                columns,
                columnCount: columns.length,
                rowCount: undefined
              });
              // Also stash the columns in the ref so `startViewRun` can
              // read them synchronously, even before React re-renders with
              // the hydrated `discoveredSchemas`.
              hydratedColumnsRef.current.set(`${schemaName}.${tableName}`, columns);
            }
          }
        }

        if (hydratedBySchema.size > 0) {
          const merged = stateRef.current.discoveredSchemas.map((schema) => {
            const hydratedTables = hydratedBySchema.get(schema.schema);
            if (!hydratedTables) return schema;
            const mergedTables = schema.tables.map((t) => hydratedTables.get(t.name) ?? t);
            return { ...schema, tables: mergedTables };
          });
          orchestrator.hydrateDiscoveredSchemas(merged);
        }
      } catch (err) {
        // Non-fatal: the builder will fall back to a DESCRIBE round-trip
        // when it can't find pre-fetched columns in `discoveredSchemas`.
        console.warn("Failed to refresh discovered schemas after re-sync:", err);
      }
    },
    [projectId, branchName, orchestrator]
  );

  const buildModelConfig = useCallback(() => {
    const { llmModel, llmVendor, llmModelRef, llmProvider } = orchestrator.state;
    if (llmModel && llmVendor && llmModelRef && llmProvider) {
      return {
        name: llmModel,
        vendor: llmVendor,
        model_ref: llmModelRef,
        key_var: LLM_KEY_VAR[llmProvider]
      };
    }
    return undefined;
  }, [orchestrator.state]);

  // Start a single build phase run (config, agent, or app).
  // Returns the new run ID so the caller can immediately reconnect the SSE stream.
  const startBuildPhase = useCallback(
    async (phase: BuildPhase): Promise<string> => {
      const { selectedTables, warehouseType } = orchestrator.state;
      if (selectedTables.length === 0) throw new Error("No tables selected");

      // Phase labels double as the user-facing thread title for each builder
      // run — keep them short and specific so the thread list reads cleanly.
      const phaseLabel: Record<BuildPhase, string> = {
        semantic_layer: "Build semantic model",
        config: "Update config.yml",
        agent: "Create analytics agent"
      };

      const onboardingContext = {
        tables: selectedTables,
        warehouse_type: warehouseType ?? "bigquery",
        step: phase,
        model_config: buildModelConfig()
      };

      const threadId = await ensureThread(phaseLabel[phase]);
      const model = builderModel ?? orchestrator.state.llmModel;
      const runBody = {
        agent_id: "__builder__",
        question: phaseLabel[phase],
        thread_id: threadId,
        domain: "builder",
        onboarding_context: onboardingContext,
        auto_accept: true,
        ...(model && { model })
      };
      const run = await AnalyticsService.createRun(
        projectId,
        runBody as typeof runBody & { agent_id: string; question: string }
      );

      orchestrator.startPhase(phase, threadId, run.run_id);

      return run.run_id;
    },
    [projectId, builderModel, orchestrator, ensureThread, buildModelConfig]
  );

  // Start a single semantic view run for one table.
  // Returns the run ID.
  const startViewRun = useCallback(
    async (table: string): Promise<string> => {
      const { warehouseType, discoveredSchemas, llmModel } = stateRef.current;

      // Look up pre-fetched column info from schema discovery to avoid
      // DESCRIBE round-trip. Prefer the synchronous ref populated by the
      // most recent `resyncWithSelectedTables` — at this point in the call
      // chain React hasn't re-rendered yet, so `discoveredSchemas` is still
      // the pre-hydration (empty-columns) state. Falling back to state
      // covers the (rare) case where resync didn't run first.
      const tableSchema = (() => {
        const refCols = hydratedColumnsRef.current.get(table);
        if (refCols && refCols.length > 0) {
          return refCols.map((c) => ({ name: c.name, column_type: c.type }));
        }
        for (const schema of discoveredSchemas) {
          const tableName = table.split(".").pop() ?? table;
          const match = schema.tables.find((t) => t.name === tableName || t.name === table);
          if (match && match.columns.length > 0) {
            return match.columns.map((c) => ({ name: c.name, column_type: c.type }));
          }
        }
        return undefined;
      })();

      const onboardingContext = {
        tables: [table],
        warehouse_type: warehouseType ?? "bigquery",
        step: "semantic_view" as const,
        model_config: buildModelConfig(),
        ...(tableSchema && { table_schema: tableSchema })
      };

      const threadId = await ensureThread(`Create view for ${table}`);
      const model = builderModel ?? llmModel;
      const runBody = {
        agent_id: "__builder__",
        question: `Create semantic view for ${table}`,
        thread_id: threadId,
        domain: "builder",
        onboarding_context: onboardingContext,
        auto_accept: true,
        ...(model && { model })
      };
      const run = await AnalyticsService.createRun(
        projectId,
        runBody as typeof runBody & { agent_id: string; question: string }
      );

      orchestrator.startViewRun(table, run.run_id);

      return run.run_id;
    },
    [projectId, builderModel, orchestrator, ensureThread, buildModelConfig]
  );

  // Upload CSV / Parquet files into the workspace for the DuckDB onboarding
  // step. On success, records the uploaded paths + subdir in orchestrator state
  // so "Start over" can clean them up and so the caller can submit a DuckDB
  // warehouse config pointed at the uploaded directory.
  const uploadWarehouseFiles = useCallback(
    async (files: File[]): Promise<{ subdir: string; files: string[] } | null> => {
      if (!projectId || files.length === 0) return null;
      try {
        const result = await OnboardingService.uploadWarehouseFiles(projectId, files);
        orchestrator.setUploadedWarehouseFiles(
          [...(orchestrator.state.uploadedWarehouseFiles ?? []), ...result.files],
          result.subdir
        );
        if (result.skipped.length > 0) {
          const names = result.skipped.map((s) => s.name).join(", ");
          toast.warning(`Skipped ${result.skipped.length} file(s): ${names}`);
        }
        return { subdir: result.subdir, files: result.files };
      } catch (err) {
        const message =
          err && typeof err === "object" && "response" in err
            ? ((err as { response?: { data?: { error?: string } | string } }).response?.data ??
              null)
            : null;
        const detail =
          typeof message === "string"
            ? message
            : message && typeof message === "object" && "error" in message
              ? message.error
              : err instanceof Error
                ? err.message
                : "Upload failed.";
        toast.error(`Failed to upload files: ${detail}`);
        return null;
      }
    },
    [projectId, orchestrator]
  );

  // ── GitHub / Demo mode ─────────────────────────────────────────────────────

  // Fetch the missing-secrets manifest for a cloned Oxy repo (or the demo
  // workspace, which uses the same shape). DuckDB warehouses are filtered out
  // because they're file-backed and don't take credentials — `password_var`
  // entries on a DuckDB warehouse are seed-data noise we shouldn't prompt for.
  const fetchGithubSetup = useCallback(async () => {
    const raw = await OnboardingService.getGithubSetup(projectId);
    const setup = {
      ...raw,
      warehouses: raw.warehouses.filter((w) => w.dialect.toLowerCase() !== "duckdb")
    };
    orchestrator.setGithubSetup(setup);
    return setup;
  }, [projectId, orchestrator]);

  // Save a single LLM key secret and advance the cursor. Empty value = skip.
  // For Anthropic / OpenAI vars we validate the key against the provider
  // first so the user catches typos before completing onboarding — same
  // gate as the blank-workspace flow. Vendors we can't probe (Google,
  // Ollama) skip validation and persist directly.
  const saveGithubLlmKey = useCallback(
    async (varName: string, value: string) => {
      const trimmed = value.trim();

      // Empty submission = "skip this key"; advance without touching secrets.
      if (trimmed.length === 0) {
        orchestrator.advanceGithubLlmKey();
        return;
      }

      const keyVar = orchestrator.state.githubSetup?.missing_llm_key_vars.find(
        (k) => k.var_name === varName
      );
      const probeProvider = keyVar ? vendorToProbeProvider(keyVar.vendor) : null;

      if (probeProvider) {
        orchestrator.startGithubLlmKeyTest();
        try {
          const result = await OnboardingService.testLlmKey(projectId, probeProvider, trimmed);
          if (!result.success) {
            orchestrator.failGithubLlmKeyTest(
              result.message ??
                "We couldn't verify that API key with the provider. Please double-check the value."
            );
            return;
          }
        } catch (err) {
          console.warn("[onboarding] GitHub LLM key validation request failed", err);
          orchestrator.failGithubLlmKeyTest(formatLlmKeyValidationError(err));
          return;
        }
      }

      try {
        await SecretService.createSecret(projectId, {
          name: varName,
          value: trimmed,
          description: `Provided during GitHub onboarding`
        });
      } catch (createErr) {
        // Update if it already exists.
        try {
          const { secrets } = await SecretService.listSecrets(projectId);
          const existing = secrets.find((s) => s.name === varName);
          if (existing) {
            await SecretService.updateSecret(projectId, existing.id, { value: trimmed });
          } else {
            console.warn(`[onboarding] Failed to save ${varName}`, createErr);
            toast.error(`Failed to save ${varName}.`);
          }
        } catch (updateErr) {
          console.warn(`[onboarding] Failed to save ${varName}`, updateErr);
          toast.error(`Failed to save ${varName}.`);
        }
      }
      orchestrator.advanceGithubLlmKey();
    },
    [projectId, orchestrator]
  );

  // Save warehouse secrets, run the connection test + sync, then advance.
  // `values` keyed by var_name. Empty values are treated as "skip this field"
  // — if the user skips everything we mark the warehouse as skipped (no test,
  // no sync). On save / sync failure we stay on the same warehouse with an
  // inline error so the user can fix typos and retry, rather than silently
  // advancing past a broken connection.
  const saveGithubWarehouseCreds = useCallback(
    async (warehouse: GithubSetupWarehouse, values: Record<string, string>) => {
      const provided = Object.entries(values).filter(([, v]) => v.trim().length > 0);

      // No values submitted → treat as skipped entirely.
      if (provided.length === 0) {
        orchestrator.advanceGithubWarehouse(warehouse.name, "skipped");
        return;
      }

      orchestrator.startGithubWarehouseTest();

      // Persist each provided secret.
      try {
        const { secrets } = await SecretService.listSecrets(projectId);
        for (const [varName, value] of provided) {
          const trimmed = value.trim();
          const existing = secrets.find((s) => s.name === varName);
          if (existing) {
            await SecretService.updateSecret(projectId, existing.id, { value: trimmed });
          } else {
            await SecretService.createSecret(projectId, {
              name: varName,
              value: trimmed,
              description: `Provided during GitHub onboarding (${warehouse.name})`
            });
          }
        }
      } catch (err) {
        console.warn("[onboarding] Failed to save warehouse secrets", err);
        const message = err instanceof Error ? err.message : "Failed to save credentials.";
        orchestrator.failGithubWarehouseTest(
          `Failed to save credentials for ${warehouse.name}: ${message}`
        );
        return;
      }

      // Sync the warehouse. This both exercises the connection (verifies the
      // credentials just saved actually work) and populates the semantic
      // model metadata, so the user can ask questions immediately without
      // running `oxy build`. A sync failure means either bad creds or a
      // reachability problem — surface it inline so the user can correct and
      // retry instead of advancing past a broken warehouse.
      try {
        await DatabaseService.syncDatabase(projectId, branchName, warehouse.name);
      } catch (err) {
        const message = err instanceof Error ? err.message : "Sync failed.";
        orchestrator.failGithubWarehouseTest(`Could not connect to ${warehouse.name}. ${message}`);
        return;
      }

      orchestrator.advanceGithubWarehouse(warehouse.name, "success");
    },
    [projectId, branchName, orchestrator]
  );

  return {
    saveLlmKey,
    testAndSaveWarehouse,
    uploadWarehouseFiles,
    discoverSchemas,
    fetchSchemaTables,
    resyncWithSelectedTables,
    startBuildPhase,
    startViewRun,
    fetchGithubSetup,
    saveGithubLlmKey,
    saveGithubWarehouseCreds
  };
}
