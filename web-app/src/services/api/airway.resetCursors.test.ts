// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";

const post = vi.fn();
vi.mock("./axios", () => ({ apiClient: { post: (...args: unknown[]) => post(...args) } }));

import { AirwayService } from "./airway";

/**
 * The route answers `409` for two different things, and the UI must not
 * confuse them: a convergence refusal, which `force` may override, and a run
 * in flight holding the pipeline lease, which it may not. Read as a refusal,
 * the second would reach the dialog with an override that cannot work — or,
 * carrying no `reasons`, throw its raw JSON into a toast.
 */
describe("AirwayService.resetCursors — the two 409s", () => {
  beforeEach(() => post.mockReset());

  const request = { pipeline_ref: "pipelines/amazon_vc.airway.yml", resources: ["vendor_sales"] };

  it("returns a held lease as its own outcome, not a refusal", async () => {
    post.mockResolvedValue({
      status: 409,
      data: {
        error: "pipeline_running",
        run_id: "run-123",
        message: "cursor reset refused: run `run-123` of this pipeline is in flight."
      }
    });

    await expect(AirwayService.resetCursors("p", request)).resolves.toEqual({
      kind: "pipeline_running",
      run_id: "run-123",
      message: "cursor reset refused: run `run-123` of this pipeline is in flight."
    });
  });

  it("still returns a convergence refusal as one", async () => {
    post.mockResolvedValue({
      status: 409,
      data: { error: "refused", reasons: ["`vendor_forecasting` appends"], refusals: [] }
    });

    await expect(AirwayService.resetCursors("p", request)).resolves.toEqual({
      kind: "refused",
      reasons: ["`vendor_forecasting` appends"]
    });
  });
});
