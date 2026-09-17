/// <reference types="node" />
// The manifest's timeout has to fit the steps that actually run by default.
// `shape_zoo` is the one that makes this non-obvious: it is in `ALL_STEPS`, so an
// unset `CANARY_STEPS` runs it, and it sends one statement per zoo case — about
// 200, all sequential. At the 60 seconds the other steps need, a run dies on the
// timeout instead of naming the case that differs, and the pager fingerprints a
// timeout rather than `canary step shape_zoo failed: …`. That is the failure
// this test exists to prevent.
//
// platform-canary carries its own pnpm-workspace.yaml, so it is outside the
// root workspace `vitest` walks; CI runs this file in the `canary-tests` job
// (.github/workflows/ci.yaml), from the canary's own lockfile. The floor below
// is also restated in shell in the `canary-zoo` job there, so a PR that lowers
// it fails even when the canary's toolchain cannot be installed; change one
// and change the other. What only this file can check is the part shell
// cannot read: that `shape_zoo` is in `ALL_STEPS` at all — the premise the
// floor is sized from — and that the zoo is still the ~200 cases the README
// quotes. Run it with `pnpm test` from this app's directory.
// The `node` reference is needed because functions/tsconfig.json sets `types: []`.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { ShapeZoo } from "./shape-zoo";
import zooJson from "./shape-zoo.json";
import { ALL_STEPS } from "./steps";

const MANIFEST = new URL("../oxy-app.json", import.meta.url);

interface Manifest {
  functions: Record<string, { timeoutSeconds?: number }>;
}

const manifest = JSON.parse(readFileSync(MANIFEST, "utf8")) as Manifest;
const zoo = zooJson as ShapeZoo;

/**
 * What one `shape_zoo` run sends: a read per case on each plane, plus a create
 * and a count on each. DuckDB cases are in the zoo but the canary does not run
 * them — it reads ClickHouse through `ctx.warehouse` and Postgres through
 * `ctx.oltp`.
 */
const ZOO_STATEMENTS = zoo.engines.clickhouse.cases.length + zoo.engines.postgres.cases.length + 4;

describe("the canary function's timeout", () => {
  it("is sized for shape_zoo, which runs unless CANARY_STEPS drops it", () => {
    expect(ALL_STEPS).toContain("shape_zoo");
    expect(
      manifest.functions.canary.timeoutSeconds,
      `shape_zoo sends ~${ZOO_STATEMENTS} sequential statements; 60s is not enough for that`
    ).toBeGreaterThanOrEqual(180);
  });

  it("keeps the README's ~200 statements honest", () => {
    // The README quotes this number and sizes the timeout from it. If the zoo
    // grows well past it, both want revisiting rather than drifting quietly.
    expect(ZOO_STATEMENTS).toBeGreaterThan(150);
    expect(ZOO_STATEMENTS).toBeLessThan(250);
  });
});
