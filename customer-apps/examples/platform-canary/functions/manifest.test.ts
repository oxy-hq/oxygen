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
  slug: string;
  functions: Record<
    string,
    {
      timeoutSeconds?: number;
      webhook?: { secretVar?: string; signatureHeader?: string; encoding?: string };
    }
  >;
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

describe("the external probe's contract", () => {
  // An external uptime monitor (oxy-hq/infrastructure, allquiet/uptime/
  // monitors.tf) POSTs to the anonymous webhook route on `echo`. It lives in
  // another repo's terraform and cannot fail this build, so the pieces of this
  // manifest its request is constructed from are pinned here.
  //
  // What it does NOT pin, on purpose: the response. That route answers 202 with
  // an EMPTY body — it enqueues rather than running inline — so the monitor
  // asserts the status and nothing else. A test here that asserted on a body
  // shape would be pinning a contract the monitor does not have. See echo.ts.

  it("keeps the webhook entry point the monitor is configured for", () => {
    const webhook = manifest.functions.echo?.webhook;
    expect(webhook, "echo lost its webhook block — the external probe now 404s").toBeDefined();
    expect(webhook?.secretVar).toBe("CANARY_PROBE_SECRET");
    expect(
      webhook?.signatureHeader,
      "the monitor sends this exact header; renaming it here 401s the probe"
    ).toBe("x-oxy-probe-signature");
    expect(webhook?.encoding, "the monitor's precomputed signature is hex").toBe("hex");
  });

  it("keeps the URL the monitor is pointed at", () => {
    // The monitor's URL is /api/webhooks/apps/<org>/<slug>/<function>. The org
    // is deployment state, but the app slug and the function name are declared
    // right here, and changing either silently 404s a check nobody is watching
    // for silence.
    expect(manifest.slug).toBe("platform-canary");
    expect(Object.keys(manifest.functions)).toContain("echo");
  });
});
