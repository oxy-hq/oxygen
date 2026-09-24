/// <reference types="node" />
// The SDK's zoo copy and digest match fixtures/data-shapes/zoo.json at the
// repo root — the canary's `shape-zoo-sync.test.ts`, for the second copy.
// Only this test reads outside the package, by relative path; the published
// entry carries the copy. The `canary-zoo` CI job restates both checks in
// shell so a stale copy fails on a path with no install.

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { ZOO_SHA256 } from "./shape-zoo-sha256";
import { ZOO, zooTableName } from "./zoo";

const FIXTURE = new URL("../../../../fixtures/data-shapes/zoo.json", import.meta.url);
const COPY = new URL("./shape-zoo.json", import.meta.url);
const SYNC = "run `node scripts/data-shapes/sync-canary-zoo.mjs` from the repo root";

describe("the SDK's shape zoo", () => {
  it("is a byte copy of fixtures/data-shapes/zoo.json", () => {
    expect(readFileSync(COPY).equals(readFileSync(FIXTURE)), `stale copy: ${SYNC}`).toBe(true);
  });

  it("carries the fixture's SHA-256", () => {
    const want = createHash("sha256").update(readFileSync(FIXTURE)).digest("hex");
    expect(ZOO_SHA256, `stale digest: ${SYNC}`).toBe(want);
  });

  it("names the zoo table from the digest, as the canary does", () => {
    expect(zooTableName()).toBe(`oxy_shape_zoo_${ZOO_SHA256.slice(0, 8)}`);
  });

  it("is the three-engine zoo the spec describes", () => {
    expect(Object.keys(ZOO.engines).sort()).toEqual(["clickhouse", "duckdb", "postgres"]);
    const total = Object.values(ZOO.engines).reduce((n, e) => n + e.cases.length, 0);
    expect(total).toBeGreaterThan(200);
    expect(ZOO.engines.postgres.cases.every((c) => "oltp" in c.expect)).toBe(true);
  });
});
