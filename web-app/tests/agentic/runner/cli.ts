import { existsSync, readdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { config as loadEnv } from "dotenv";
import { ensureBackend, resolveBaseUrl, resolveHealthUrl } from "./backend";
import { caseBudgetUsd, createMeter } from "./budget";
import {
  defaultCachePath,
  defaultStagingPath,
  readStagedFromStdin,
  runAcceptHealing,
  runCheckCoverage,
  runDryRun,
  runInspectCache,
  runList,
  runWatch
} from "./cli-modes";
import { ensureFrontend } from "./frontend";
import {
  appendToStepSummary,
  buildMarkdown,
  printSummary,
  summarizeResults,
  writeMarkdown,
  writeResults
} from "./reporter";
import type { Runtime, RuntimeContext } from "./runtimes/interface";
import { runScaffold } from "./scaffold";
import { ensureSession } from "./session";
import {
  type BackendMode,
  type CaseResult,
  type CaseRunResult,
  emptyJudgeUsage,
  emptyTokens,
  type FlowCase,
  type FlowResult,
  type FlowTest,
  type RunResults
} from "./types";
import { loadFlow } from "./yaml-loader";

const __dirname = dirname(fileURLToPath(import.meta.url));
const WEB_APP_DIR = resolve(__dirname, "..", "..", "..");
// dotenv won't override existing process env vars, so the first load wins
// for any var present in both files. Loading `.env.local` first matches
// Vite's convention: a developer's local secrets override any committed
// `.env` defaults.
loadEnv({ path: resolve(WEB_APP_DIR, ".env.local") });
loadEnv({ path: resolve(WEB_APP_DIR, ".env") });

const FLOWS_DIR = resolve(__dirname, "..", "flows");
const RESULTS_DIR = resolve(__dirname, "..", ".results");

type CliMode =
  | "run"
  | "dry-run"
  | "list"
  | "inspect-cache"
  | "watch"
  | "check-coverage"
  | "accept-healing"
  | "scaffold";

interface CliArgs {
  mode: CliMode;
  /**
   * Zero or more positional filters. A flow matches if its filename
   * contains ANY of the listed substrings. Used by the CI matrix to
   * group flows into domain buckets: `pnpm test:agentic chat-ask
   * chat-panel-agent-switch` runs both, nothing else.
   */
  globs: string[];
  tag?: string;
  output?: string;
  noAutoBackend: boolean;
  noAutoFrontend: boolean;
  headed: boolean;
  debug: boolean;
  /** For --check-coverage: staged file paths (one per arg or via --staged stdin). */
  stagedPaths?: string[];
  /** For --scaffold: the source path to infer the surface from. */
  scaffoldFrom?: string;
  /** For --scaffold: the feature name (used as the YAML filename). */
  scaffoldName?: string;
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));

  if (args.mode === "list") {
    runList({ flows: discoverFlows(args.globs), tag: args.tag });
    return;
  }
  if (args.mode === "dry-run") {
    runDryRun({ flows: discoverFlows(args.globs), tag: args.tag });
    return;
  }
  if (args.mode === "inspect-cache") {
    runInspectCache({ cachePath: defaultCachePath(__dirname) });
    return;
  }
  if (args.mode === "check-coverage") {
    runCheckCoverage({
      surfacesPath: resolve(FLOWS_DIR, "_surfaces.yml"),
      stagedPaths: args.stagedPaths ?? []
    });
    return;
  }
  if (args.mode === "watch") {
    await runWatch({
      flowsDir: FLOWS_DIR,
      rerun: () => runDryRun({ flows: discoverFlows(args.globs), tag: args.tag })
    });
    return;
  }
  if (args.mode === "accept-healing") {
    // accept-healing promotes staging entries for ONE flow. If multiple
    // positional args were given, the first wins (matches the prior
    // single-string semantics — multiple args here doesn't make sense).
    runAcceptHealing({
      stagingPath: defaultStagingPath(__dirname),
      cachePath: defaultCachePath(__dirname),
      flowFilter: args.globs[0]
    });
    return;
  }
  if (args.mode === "scaffold") {
    if (!args.scaffoldName || !args.scaffoldFrom) {
      console.error("usage: pnpm test:agentic --scaffold <name> --from <component-path>");
      process.exit(2);
    }
    runScaffold({
      featureName: args.scaffoldName,
      fromPath: resolve(args.scaffoldFrom),
      outPath: resolve(FLOWS_DIR, `${args.scaffoldName}.flow.test.yml`)
    });
    return;
  }

  const apiKey = process.env.ANTHROPIC_API_KEY;
  if (!apiKey) {
    console.error("ERROR: ANTHROPIC_API_KEY is required (export it or set in web-app/.env.local)");
    process.exit(2);
  }

  const flows = discoverFlows(args.globs);
  if (flows.length === 0) {
    console.error(
      `no flows matched ${args.globs.length > 0 ? `'${args.globs.join(", ")}'` : `in ${FLOWS_DIR}`}`
    );
    process.exit(2);
  }
  console.log(
    `[runner] discovered ${flows.length} flow(s): ${flows.map((f) => f.name).join(", ")}`
  );

  const runtime = await loadRuntime();
  console.log(`[runner] using runtime: ${runtime.name}`);

  // Read before anything is spawned: a mistyped limit should cost a second,
  // not a backend boot.
  const budgetUsd = caseBudgetUsd();
  console.log(
    budgetUsd === undefined
      ? "[runner] per-case budget: none (AGENTIC_CASE_BUDGET_USD)"
      : `[runner] per-case budget: $${budgetUsd.toFixed(2)}`
  );

  const mode = pickBackendMode(flows);
  // Propagate the resolved URLs into the environment so anything reading
  // them downstream (the bespoke runtime's Playwright context, the seed
  // fixtures, etc.) sees the same target the backend health-check used.
  // Don't clobber an explicit user override.
  process.env.OXY_BASE_URL = resolveBaseUrl(mode);
  process.env.OXY_HEALTH_URL = resolveHealthUrl(mode);
  console.log(
    `[runner] backend mode: ${mode} (base=${process.env.OXY_BASE_URL} health=${process.env.OXY_HEALTH_URL})`
  );

  const backend = args.noAutoBackend
    ? { spawned: false, shutdown: async () => {} }
    : await ensureBackend({ mode });
  // Enterprise mode needs a signed-in browser and a workspace prefix whether
  // or not the runner started the backend (CI passes --no-auto-backend).
  await ensureSession(mode);
  const frontend = args.noAutoFrontend
    ? { spawned: false, shutdown: async () => {} }
    : await ensureFrontend();

  const startedAt = new Date().toISOString();
  const startMs = Date.now();
  const flowResults: FlowResult[] = [];

  try {
    for (const flow of flows) {
      const tag = args.tag;
      const filtered = tag ? flow.cases.filter((c) => c.tags.includes(tag)) : flow.cases;
      if (filtered.length === 0) continue;
      flowResults.push(await runFlow(flow, filtered, runtime, apiKey, args, budgetUsd));
    }
  } finally {
    if (frontend.spawned) await frontend.shutdown();
    if (backend.spawned) await backend.shutdown();
  }

  const raw: RunResults = {
    runtime: runtime.name,
    started_at: startedAt,
    duration_ms: Date.now() - startMs,
    cost_usd: 0,
    pricing_as_of: "",
    flows: flowResults
  };
  const results = summarizeResults(raw);

  printSummary(results);

  // Always write a timestamped pair under .results/ so devs and CI both have
  // an artifact to inspect even when --output isn't passed.
  const stamp = startedAt.replace(/[:.]/g, "-");
  const defaultJson = resolve(RESULTS_DIR, `${stamp}.json`);
  const defaultMd = resolve(RESULTS_DIR, `${stamp}.md`);
  const markdown = buildMarkdown(results);
  writeResults(results, defaultJson);
  writeMarkdown(markdown, defaultMd);
  console.log(`[runner] wrote results to ${defaultJson}`);
  console.log(`[runner] wrote markdown to ${defaultMd}`);

  if (args.output) {
    writeResults(results, args.output);
    console.log(`[runner] wrote results to ${args.output}`);
  }

  appendToStepSummary(markdown);

  const allPassed = results.flows.every((f) =>
    f.cases.every((c) => c.runs.every((r) => r.passed && r.expect_results.every((e) => e.passed)))
  );
  process.exit(allPassed ? 0 : 1);
}

async function runFlow(
  flow: FlowTest,
  cases: FlowCase[],
  runtime: Runtime,
  apiKey: string,
  args: CliArgs,
  budgetUsd: number | undefined
): Promise<FlowResult> {
  const caseResults: CaseResult[] = [];

  for (const c of cases) {
    const runs: CaseRunResult[] = [];
    for (let i = 0; i < flow.settings.runs; i++) {
      const ctx: RuntimeContext = {
        flow,
        testCase: c,
        apiKey,
        debug: args.debug,
        headless: !args.headed,
        // One meter per case run: a stopped meter stays stopped, and one case
        // running away must not take the budget of the cases after it.
        meter: budgetUsd === undefined ? undefined : createMeter(budgetUsd)
      };
      const start = Date.now();
      try {
        const result = await runtime.runCase(ctx);
        runs.push({ ...result, duration_ms: result.duration_ms || Date.now() - start });
      } catch (err) {
        runs.push({
          passed: false,
          duration_ms: Date.now() - start,
          step_count: 0,
          tokens: emptyTokens(),
          cache_hits: [],
          expect_results: [],
          step_debug: [],
          judge_usage: emptyJudgeUsage(),
          cost_usd: 0,
          error: err instanceof Error ? err.message : String(err)
        });
      }
    }
    caseResults.push({ name: c.name, runs });
  }

  return { name: flow.name, file: flow.file, cases: caseResults };
}

async function loadRuntime(): Promise<Runtime> {
  const tries = [{ mod: "./runtimes/bespoke", export: "bespokeRuntime" }];
  for (const t of tries) {
    try {
      const mod = (await import(t.mod)) as Record<string, Runtime | undefined>;
      const r = mod[t.export];
      if (r) return r;
    } catch {
      // module not present on this branch; try next
    }
  }
  throw new Error(
    "no runtime found. Add a runtime module under web-app/tests/agentic/runner/runtimes/ that exports a `Runtime` (see runtimes/interface.ts) and register it here."
  );
}

function pickBackendMode(flows: FlowTest[]): BackendMode {
  const modes = new Set(flows.map((f) => f.settings.backend_mode));
  if (modes.size > 1) {
    const grouped = flows
      .map((f) => `  ${f.settings.backend_mode}: ${f.name}`)
      .sort()
      .join("\n");
    throw new Error(
      "agentic runner: cannot run flows with mixed backend_mode in a single invocation.\n" +
        "Each backend mode requires a different oxy boot configuration. Filter to one\n" +
        "mode at a time (e.g. `pnpm test:agentic builder-edits-app`).\n\n" +
        `Loaded flows:\n${grouped}`
    );
  }
  return [...modes][0] ?? "cloud";
}

function discoverFlows(globs: string[] = []): FlowTest[] {
  if (!existsSync(FLOWS_DIR)) return [];
  const files = readdirSync(FLOWS_DIR)
    .filter((f) => f.endsWith(".flow.test.yml"))
    .filter((f) => globs.length === 0 || globs.some((g) => f.includes(g)))
    .map((f) => join(FLOWS_DIR, f));
  return files.map(loadFlow);
}

function parseArgs(argv: string[]): CliArgs {
  // Truthy-by-presence semantics with explicit "0"/"false"/"" opt-out.
  // `HEADED=0 pnpm test:agentic ...` should NOT enable headed mode.
  const truthy = (v: string | undefined) =>
    v !== undefined && v !== "" && v !== "0" && v !== "false";
  const out: CliArgs = {
    mode: "run",
    globs: [],
    noAutoBackend: false,
    noAutoFrontend: false,
    headed: truthy(process.env.HEADED),
    debug: truthy(process.env.DEBUG)
  };
  const stagedPaths: string[] = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--tag") out.tag = argv[++i];
    else if (a === "--output") out.output = argv[++i];
    else if (a === "--no-auto-backend") out.noAutoBackend = true;
    else if (a === "--no-auto-frontend") out.noAutoFrontend = true;
    else if (a === "--headed") out.headed = true;
    else if (a === "--debug") out.debug = true;
    else if (a === "--dry-run") out.mode = "dry-run";
    else if (a === "--list") out.mode = "list";
    else if (a === "--inspect-cache") out.mode = "inspect-cache";
    else if (a === "--watch") out.mode = "watch";
    else if (a === "--check-coverage") out.mode = "check-coverage";
    else if (a === "--accept-healing") out.mode = "accept-healing";
    else if (a === "--scaffold") {
      out.mode = "scaffold";
      out.scaffoldName = argv[++i];
    } else if (a === "--from") out.scaffoldFrom = argv[++i];
    else if (a === "--staged") {
      // Read NUL- or newline-separated paths from stdin (`git diff --name-only ... | xargs`).
      // Husky pre-commit hooks call this directly.
      const stdin = readStagedFromStdin();
      stagedPaths.push(...stdin);
    } else if (a === "--path") stagedPaths.push(argv[++i]);
    else if (!a.startsWith("--")) out.globs.push(a);
  }
  if (stagedPaths.length > 0) out.stagedPaths = stagedPaths;
  return out;
}

main().catch((err) => {
  console.error(err);
  process.exit(1);
});
