import type { Node } from "@xyflow/react";
import { describe, expect, it } from "vitest";

import { keepMeasured } from "./keepMeasured";

const node = (id: string, extra: Partial<Node> = {}): Node => ({
  id,
  position: { x: 0, y: 0 },
  data: {},
  ...extra
});

describe("keepMeasured", () => {
  it("carries each node's measured size onto its rebuilt object", () => {
    const prev = [node("a", { measured: { width: 120, height: 40 } }), node("b")];
    const built = [node("a", { data: { status: "done" } }), node("b"), node("c")];

    const next = keepMeasured(prev, built);

    expect(next.map((n) => n.measured)).toEqual([{ width: 120, height: 40 }, undefined, undefined]);
    expect(next[0].data).toEqual({ status: "done" });
  });

  it("drops sizes for nodes that are gone", () => {
    const prev = [node("gone", { measured: { width: 1, height: 1 } })];
    expect(keepMeasured(prev, [node("new")])[0].measured).toBeUndefined();
  });
});
