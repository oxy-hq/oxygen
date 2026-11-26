import { existsSync, readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { parse } from "yaml";
import { loadFlow } from "./yaml-loader";

/**
 * The suite's own shape, checked for free.
 *
 * Everything here is a mistake that costs a CI run to find and says nothing
 * about the product when it does. None of it needs a browser, a backend or an
 * API key, so it runs in the same second as the rest of `vitest`.
 *
 * The one that prompted the file: the CI matrix in
 * `.github/workflows/agentic-tests.yaml` names flows by FILENAME in an inline
 * JSON string, and the runner selects a flow whose filename merely CONTAINS a
 * positional argument. So a renamed flow leaves a bucket matching nothing, and
 * a bucket that matches nothing is not an error — it is a green job that ran no
 * test. Recentring the matrix on the custom-app and pipeline buckets rewrote
 * every one of those names at once, which is exactly when a typo gets in.
 */

const FLOWS_DIR = resolve(__dirname, "../flows");
const WORKFLOW = resolve(__dirname, "../../../../.github/workflows/agentic-tests.yaml");

const flowFiles = (): string[] =>
  readdirSync(FLOWS_DIR)
    .filter((f) => f.endsWith(".flow.test.yml"))
    .sort();

const stem = (file: string) => file.replace(/\.flow\.test\.yml$/, "");

interface BudgetEntry {
  flow: string;
  cold_usd?: number;
  warm_usd?: number;
}

const budgets = (): BudgetEntry[] =>
  parse(readFileSync(resolve(FLOWS_DIR, "_budgets.yml"), "utf-8")) as BudgetEntry[];

/** The bucket matrix, read out of the workflow's inline `ALL='[…]'` JSON. */
interface Bucket {
  name: string;
  flows: string;
  mode: string;
  seed_path?: string;
  identity?: string;
}

/** The flow filenames one bucket names, as the workflow space-separates them. */
const bucketFlows = (bucket: Bucket): string[] => bucket.flows.split(/\s+/).filter(Boolean);

const flowMode = (file: string) => loadFlow(resolve(FLOWS_DIR, file)).settings.backend_mode;

function buckets(): Bucket[] {
  const yaml = readFileSync(WORKFLOW, "utf-8");
  // The matrix is a shell single-quoted JSON literal inside a `run:` block.
  // Matching it textually keeps this test honest about the file CI reads,
  // rather than about a copy of the list kept here.
  const json = /ALL='(\[[\s\S]*?\])'/.exec(yaml)?.[1];
  if (!json) throw new Error(`could not find the ALL='[…]' bucket matrix in ${WORKFLOW}`);
  return JSON.parse(json) as Bucket[];
}

describe("every committed flow", () => {
  it("loads through the runner's own loader", () => {
    const failures: string[] = [];
    for (const file of flowFiles()) {
      try {
        const flow = loadFlow(resolve(FLOWS_DIR, file));
        expect(flow.cases.length, `${file} declares no cases`).toBeGreaterThan(0);
      } catch (err) {
        failures.push(`${file}: ${err instanceof Error ? err.message : String(err)}`);
      }
    }
    expect(failures, failures.join("\n")).toEqual([]);
  });

  it("has a cost ceiling, and no ceiling names a flow that is gone", () => {
    const stems = new Set(flowFiles().map(stem));
    const budgeted = new Set(budgets().map((b) => b.flow));

    // A flow with no ceiling is a flow the reporter cannot warn about, which
    // is how a cost regression ships unnoticed.
    expect([...stems].filter((s) => !budgeted.has(s))).toEqual([]);
    // An orphan ceiling is a flow someone deleted and a line nobody did.
    expect([...budgeted].filter((b) => !stems.has(b))).toEqual([]);
  });

  it("states both a cold and a warm ceiling in every entry", () => {
    const partial = budgets()
      .filter((b) => typeof b.cold_usd !== "number" || typeof b.warm_usd !== "number")
      .map((b) => b.flow);
    expect(partial).toEqual([]);
  });
});

describe("the CI bucket matrix", () => {
  it("names only flows that exist", () => {
    const present = new Set(flowFiles());
    const missing = buckets().flatMap((bucket) =>
      bucketFlows(bucket)
        .filter((file) => !present.has(file))
        .map((file) => `${bucket.name} → ${file}`)
    );
    expect(missing, `bucket entries matching no flow file:\n${missing.join("\n")}`).toEqual([]);
  });

  it("names each flow in at most one bucket", () => {
    const seen = new Map<string, string>();
    const twice: string[] = [];
    for (const bucket of buckets()) {
      for (const file of bucketFlows(bucket)) {
        const first = seen.get(file);
        if (first) twice.push(`${file}: ${first} and ${bucket.name}`);
        else seen.set(file, bucket.name);
      }
    }
    expect(twice, twice.join("\n")).toEqual([]);
  });

  it("keeps every bucket on one backend mode, as the runner requires", () => {
    // Two ways to get this wrong, and the same list reports both: `pnpm
    // test:agentic` errors loudly on a mixed-mode invocation, so a bucket that
    // mixes them cannot run at all — and a bucket whose declared mode is not
    // its flows' mode boots the wrong backend for them.
    const wrong = buckets().flatMap((bucket) => {
      const modes = [...new Set(bucketFlows(bucket).map(flowMode))];
      return [
        ...(modes.length > 1 ? [`${bucket.name} mixes modes: ${modes.join(" + ")}`] : []),
        ...modes
          .filter((mode) => mode !== bucket.mode)
          .map((mode) => `${bucket.name} declares ${bucket.mode}, flow is ${mode}`)
      ];
    });
    expect(wrong, wrong.join("\n")).toEqual([]);
  });

  it("gives every bucket a seed path that exists and an identity to sign in as", () => {
    const repo = resolve(__dirname, "../../../..");
    const wrong: string[] = [];
    for (const bucket of buckets()) {
      if (!bucket.identity) wrong.push(`${bucket.name}: no identity`);
      if (!bucket.seed_path) wrong.push(`${bucket.name}: no seed_path`);
      else if (!existsSync(resolve(repo, bucket.seed_path))) {
        wrong.push(`${bucket.name}: seed_path ${bucket.seed_path} is not in the repo`);
      }
    }
    expect(wrong, wrong.join("\n")).toEqual([]);
  });

  it("keeps warehouse-querying flows out of a fixture with no warehouse", () => {
    // The finding this test exists for, because the seed does NOT fail on it.
    // `examples/config.yml` declares `local` as `type: airhouse_managed`, and
    // the agentic job runs no Airhouse — `provision_tenants` prints
    // "skipping Airhouse tenants — no Airhouse", returns false, and the seed
    // step stays GREEN with no data loaded. A flow that then executes a query
    // fails on an empty warehouse, looking exactly like a product bug.
    // `demo_project/config.yml`'s `local` is `type: duckdb, dataset: .db/`, a
    // file in the checkout, so that fixture can answer a query.
    //
    // A case that runs a query carries the `warehouse-query` tag. Any bucket
    // on a fixture whose `local` is not file-backed may not name such a flow.
    const FILE_BACKED = new Set(["demo_project"]);
    const offenders = buckets()
      .filter((bucket) => !FILE_BACKED.has(bucket.seed_path ?? ""))
      .flatMap((bucket) =>
        bucketFlows(bucket)
          .filter((file) =>
            loadFlow(resolve(FLOWS_DIR, file)).cases.some((c) =>
              (c.tags ?? []).includes("warehouse-query")
            )
          )
          .map((file) => `${bucket.name} (seeds ${bucket.seed_path}) → ${file}`)
      );
    expect(
      offenders,
      `these flows execute a query but their bucket's fixture has no warehouse:\n${offenders.join("\n")}`
    ).toEqual([]);
  });

  it("covers the product's core surfaces", () => {
    // The claim this recentring exists to make, kept honest: whatever the
    // buckets are called, the matrix has to run at least one flow about the
    // served custom app, one about its console, and one about the pipeline.
    // Until 2026-11-26 it ran none of the three.
    const inCi = buckets().flatMap(bucketFlows);
    const targets = new Set(inCi.map((f) => loadFlow(resolve(FLOWS_DIR, f)).target));
    expect(
      targets.has("custom-app"),
      `no custom-app flow in CI; buckets run ${inCi.join(" ")}`
    ).toBe(true);
    expect(targets.has("pipeline"), `no pipeline flow in CI; buckets run ${inCi.join(" ")}`).toBe(
      true
    );
  });
});
