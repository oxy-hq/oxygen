// @vitest-environment jsdom

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ArtifactItem } from "@/hooks/analyticsSteps";
import { RunDbtModelsView } from "./AnalyticsArtifactViews";

// lottie-web paints a canvas at import time and jsdom has no canvas.
vi.mock("@lottiefiles/react-lottie-player", () => ({ Player: "div" }));

const run = (statuses: string[]): ArtifactItem => ({
  kind: "artifact",
  id: "tool-1",
  toolName: "run_dbt_models",
  toolInput: JSON.stringify({ project: "shop" }),
  toolOutput: JSON.stringify({
    ok: true,
    results: statuses.map((status, i) => ({ unique_id: `model.shop.m${i}`, status }))
  }),
  isStreaming: false
});

/** The text of the "Models" summary tile, e.g. "2 ok · 1 err". */
const modelsSummary = () => screen.getByText("Models").nextElementSibling?.textContent;

describe("RunDbtModelsView models summary", () => {
  afterEach(cleanup);

  it("separates the error count from the ok count with a visible dot", () => {
    render(<RunDbtModelsView item={run(["success", "success", "error", "skip"])} />);
    expect(modelsSummary()).toBe("2 ok · 1 err · 1 skip");
  });

  it("starts with the error count when nothing succeeded", () => {
    render(<RunDbtModelsView item={run(["error"])} />);
    expect(modelsSummary()).toBe("1 err");
  });
});
