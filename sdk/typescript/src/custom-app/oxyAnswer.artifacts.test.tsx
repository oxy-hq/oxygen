// @vitest-environment jsdom
//
// A SQL artifact's rows are a preview: a react run sends the first 10 of a
// result, with `rowCount` the full count. The "+N more rows" note counts from
// `rowCount`, so a 30-row result shown 10 at a time says 20 more, not 0.

import * as React from "react";
import { createRoot } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";
import { type AgentArtifact, OxyAnswer } from "./react";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const container = document.createElement("div");
document.body.appendChild(container);
const root = createRoot(container);

afterEach(async () => {
  await React.act(async () => root.render(<></>));
});

function artifact(rows: number, rowCount: number): AgentArtifact {
  return {
    type: "sql",
    id: "q1",
    source: "query_executed",
    sql: "select n from t",
    results: {
      columns: ["n"],
      rows: Array.from({ length: rows }, (_, i) => [i]),
      rowCount
    }
  };
}

async function openNote(a: AgentArtifact, maxArtifactRows?: number): Promise<string> {
  await React.act(async () =>
    root.render(
      <OxyAnswer
        answer='done'
        state='done'
        threadUrl={null}
        artifacts={[a]}
        maxArtifactRows={maxArtifactRows}
      />
    )
  );
  const toggle = container.querySelector("button");
  await React.act(async () => toggle?.click());
  return container.textContent ?? "";
}

describe("OxyAnswer SQL artifact rows", () => {
  it("counts hidden rows from rowCount, not the preview", async () => {
    expect(await openNote(artifact(10, 30))).toContain("+20 more rows");
  });

  it("says nothing more when the preview is the whole result", async () => {
    expect(await openNote(artifact(3, 3))).not.toContain("more rows");
  });

  it("counts a preview longer than the limit as before", async () => {
    expect(await openNote(artifact(12, 12), 10)).toContain("+2 more rows");
  });
});
