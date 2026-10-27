// @vitest-environment jsdom

import { AxiosError } from "axios";
import { beforeEach, describe, expect, it, vi } from "vitest";

const get = vi.fn();
const post = vi.fn();
const put = vi.fn();
vi.mock("./axios", () => ({
  apiClient: {
    get: (...args: unknown[]) => get(...args),
    post: (...args: unknown[]) => post(...args),
    put: (...args: unknown[]) => put(...args)
  }
}));

import { ExperimentService } from "./experiments";

const failure = (status: number, data: unknown) =>
  new AxiosError("Request failed", "ERR_BAD_RESPONSE", undefined, undefined, {
    status,
    data
  } as never);

beforeEach(() => {
  get.mockReset();
  post.mockReset();
  put.mockReset();
});

/**
 * `GET /experiments/{name}/result` answers two different 409s (PR 5, Task 8):
 * `{ sealed: true, state }` while the run has not reported — the platform declining
 * to peek — and `{ sealed: false, pending: true, state }` for a terminal run whose
 * report row is not written yet. Neither is a failure, and left to axios' default both
 * reject as "Request failed with status code 409". Any other conflict is a real error
 * and keeps its own message.
 */
describe("ExperimentService.getResult", () => {
  it("returns a 409 sealed body as the sealed outcome, not an error", async () => {
    get.mockResolvedValue({ status: 409, data: { sealed: true, state: "live" } });

    await expect(ExperimentService.getResult("p", "beverage_promo")).resolves.toEqual({
      kind: "sealed",
      state: "live"
    });
  });

  it("returns a 409 pending body as its own outcome, distinct from sealed", async () => {
    get.mockResolvedValue({
      status: 409,
      data: { sealed: false, pending: true, state: "reported" }
    });

    await expect(ExperimentService.getResult("p", "x")).resolves.toEqual({
      kind: "pending",
      state: "reported"
    });
  });

  it("asks axios to accept 409 (and only 2xx besides) so it is never rejected", async () => {
    get.mockResolvedValue({ status: 200, data: {} });
    await ExperimentService.getResult("p", "x");

    const config = get.mock.calls[0][1] as { validateStatus: (s: number) => boolean };
    expect(config.validateStatus(200)).toBe(true);
    expect(config.validateStatus(409)).toBe(true);
    expect(config.validateStatus(404)).toBe(false);
    expect(config.validateStatus(500)).toBe(false);
  });

  it("does not read any other 409 as sealed or pending", async () => {
    get.mockResolvedValue({ status: 409, data: "a different conflict" });
    await expect(ExperimentService.getResult("p", "x")).rejects.toThrow("a different conflict");

    get.mockResolvedValue({ status: 409, data: { sealed: false, pending: true, state: "live" } });
    await expect(ExperimentService.getResult("p", "x")).rejects.toThrow(
      "The result request was refused (409)."
    );
  });

  it("returns a 200 body as the result", async () => {
    const body = { experiment: "x", state: "reported", report: { label: "Registered result" } };
    get.mockResolvedValue({ status: 200, data: body });

    await expect(ExperimentService.getResult("p", "x")).resolves.toEqual({
      kind: "report",
      result: body
    });
  });

  it("rethrows a JSON error body's message or error field", async () => {
    get.mockRejectedValue(failure(404, { error: "no run for this experiment" }));
    await expect(ExperimentService.getResult("p", "x")).rejects.toThrow(
      "no run for this experiment"
    );

    get.mockRejectedValue(failure(503, { message: "not compiled yet; retry shortly" }));
    await expect(ExperimentService.getResult("p", "x")).rejects.toThrow(
      "not compiled yet; retry shortly"
    );
  });
});

describe("ExperimentService error bodies", () => {
  it("reads a plain-text error body — the backend's ApiError is (StatusCode, String)", async () => {
    post.mockRejectedValue(failure(400, "wave_sizes must list at least one wave"));

    await expect(
      ExperimentService.price("p", "x", {
        candidates: [
          { shape: "waves", wave_sizes: [], first_switch_on: "2030-01-01", spacing_days: 14 }
        ]
      })
    ).rejects.toThrow("wave_sizes must list at least one wave");
  });

  it("falls through to the original error when the body says nothing", async () => {
    const original = failure(500, undefined);
    get.mockRejectedValue(original);

    await expect(ExperimentService.list("p")).rejects.toBe(original);
  });
});

describe("ExperimentService permission errors", () => {
  it("turns an empty-body 403 into a readable message, not axios' generic one", async () => {
    post.mockRejectedValue(failure(403, ""));
    await expect(
      ExperimentService.abort("p", "x", { reason: "why", levers: "off" })
    ).rejects.toThrow("You don't have permission to abort this experiment.");

    post.mockRejectedValue(failure(403, undefined));
    await expect(
      ExperimentService.price("p", "x", {
        candidates: [
          { shape: "waves", wave_sizes: [2], first_switch_on: "2030-01-01", spacing_days: 14 }
        ]
      })
    ).rejects.toThrow("You don't have permission to price this experiment.");
  });

  it("keeps a 403 body's own message", async () => {
    post.mockRejectedValue(failure(403, "owners only"));
    await expect(
      ExperimentService.abort("p", "x", { reason: "why", levers: "off" })
    ).rejects.toThrow("owners only");
  });
});

describe("ExperimentService calls", () => {
  it("sends every candidate in one request and returns the request id and all proposal ids", async () => {
    post.mockResolvedValue({
      status: 202,
      data: { request_id: "req-1", proposal_ids: ["prop-1", "prop-2"] }
    });
    const request = {
      candidates: [
        {
          shape: "waves" as const,
          wave_sizes: [3, 3],
          first_switch_on: "2026-11-02",
          spacing_days: 14,
          alpha: 0.05
        },
        {
          shape: "switchback" as const,
          first_period_on: "2026-11-02",
          period_days: 7,
          pairs: 6,
          alpha: 0.2
        }
      ]
    };

    await expect(ExperimentService.price("p", "x", request)).resolves.toEqual({
      request_id: "req-1",
      proposal_ids: ["prop-1", "prop-2"]
    });
    expect(post).toHaveBeenCalledWith("/p/experiments/x/price", request);
  });

  it("encodes the experiment name into every per-experiment path", async () => {
    get.mockResolvedValue({ status: 200, data: {} });
    post.mockResolvedValue({ status: 202, data: {} });

    await ExperimentService.getRun("p", "a b");
    await ExperimentService.getProposals("p", "a b");
    await ExperimentService.abort("p", "a b", { reason: "why", levers: "off" });
    await ExperimentService.applyWriteBack("p", "a b");

    expect(get.mock.calls.map((c) => c[0])).toEqual([
      "/p/experiments/a%20b/run",
      "/p/experiments/a%20b/proposals"
    ]);
    expect(post.mock.calls.map((c) => c[0])).toEqual([
      "/p/experiments/a%20b/abort",
      "/p/experiments/a%20b/write-back/apply"
    ]);
  });

  it("sends the abort reason and lever choice as the body", async () => {
    post.mockResolvedValue({ status: 202, data: {} });

    await ExperimentService.abort("p", "x", {
      reason: "guardrail tripped by hand",
      levers: "keep"
    });

    expect(post).toHaveBeenCalledWith("/p/experiments/x/abort", {
      reason: "guardrail tripped by hand",
      levers: "keep"
    });
  });

  it("never sends a branch param on a read", async () => {
    get.mockResolvedValue({ status: 200, data: [] });

    await ExperimentService.list("p");
    await ExperimentService.getProposals("p", "x");
    await ExperimentService.getRun("p", "x");

    expect(JSON.stringify(get.mock.calls)).not.toContain("branch");
  });
});

describe("ExperimentService design versions", () => {
  const config = { experiment: "x" } as never;

  it("creates an experiment with the config as the body", async () => {
    post.mockResolvedValue({ status: 201, data: { id: "d1", version: 1 } });

    await expect(ExperimentService.create("p", config)).resolves.toEqual({ id: "d1", version: 1 });
    expect(post).toHaveBeenCalledWith("/p/experiments", { config });
  });

  it("surfaces a plain-text 409 / 422 from create", async () => {
    post.mockRejectedValue(failure(409, "an experiment named `x` already exists"));
    await expect(ExperimentService.create("p", config)).rejects.toThrow("already exists");

    post.mockRejectedValue(failure(422, "lever does not resolve"));
    await expect(ExperimentService.create("p", config)).rejects.toThrow("lever does not resolve");
  });

  it("reads the design from the encoded path", async () => {
    get.mockResolvedValue({ status: 200, data: { id: "d1", frozen_at: null } });

    await expect(ExperimentService.getDesign("p", "a b")).resolves.toEqual({
      id: "d1",
      frozen_at: null
    });
    expect(get).toHaveBeenCalledWith("/p/experiments/a%20b/design");
  });

  it("PUTs the draft with base_hash, and omits it when there is none", async () => {
    put.mockResolvedValue({ status: 200, data: { id: "d1", content_hash: "h2" } });
    await expect(ExperimentService.saveDesign("p", "x", config, "h1")).resolves.toEqual({
      id: "d1",
      content_hash: "h2"
    });
    expect(put).toHaveBeenCalledWith("/p/experiments/x/design", { config, base_hash: "h1" });

    await ExperimentService.saveDesign("p", "x", config);
    expect(put).toHaveBeenLastCalledWith("/p/experiments/x/design", { config });
  });

  it("shows the frozen / stale-hash 409 message from a save", async () => {
    put.mockRejectedValue(failure(409, "the design is frozen"));
    await expect(ExperimentService.saveDesign("p", "x", config, "h1")).rejects.toThrow(
      "the design is frozen"
    );
  });

  it("duplicates under the new name and returns the new design", async () => {
    post.mockResolvedValue({ status: 201, data: { id: "d9" } });

    await expect(ExperimentService.duplicate("p", "a b", "a_copy")).resolves.toEqual({ id: "d9" });
    expect(post).toHaveBeenCalledWith("/p/experiments/a%20b/duplicate", { name: "a_copy" });

    post.mockRejectedValue(failure(409, "an experiment named `a_copy` already exists"));
    await expect(ExperimentService.duplicate("p", "a b", "a_copy")).rejects.toThrow(
      "already exists"
    );
  });

  it("posts to the proposal's use path with no body", async () => {
    post.mockResolvedValue({ status: 200, data: { id: "d3" } });

    await ExperimentService.adoptProposal("p", "a b", "prop-1");
    expect(post).toHaveBeenCalledWith("/p/experiments/a%20b/proposals/prop-1/use");
  });

  it("registers with the content hash, and reads a 403 as a permission message", async () => {
    post.mockResolvedValue({ status: 201, data: { run_id: "r", design_id: "d1" } });
    await ExperimentService.register("p", "x", "h1");
    expect(post).toHaveBeenCalledWith("/p/experiments/x/register", { content_hash: "h1" });

    post.mockRejectedValue(failure(403, ""));
    await expect(ExperimentService.register("p", "x", "h1")).rejects.toThrow(
      "You don't have permission to register this experiment."
    );
  });

  it("posts a retrospective with the content hash, and reads a 403 as a permission message", async () => {
    post.mockResolvedValue({ status: 201, data: { run_id: "r", design_id: "d1" } });
    await ExperimentService.retrospective("p", "x", "h1");
    expect(post).toHaveBeenCalledWith("/p/experiments/x/retrospective", { content_hash: "h1" });

    post.mockRejectedValue(failure(403, ""));
    await expect(ExperimentService.retrospective("p", "x", "h1")).rejects.toThrow(
      "You don't have permission to analyse this experiment."
    );
  });
});

describe("ExperimentService interim", () => {
  it("returns a 409 not-in-progress body as a value, not an error", async () => {
    get.mockResolvedValue({
      status: 409,
      data: { in_progress: false, state: "reported", error: "the run is not live" }
    });
    await expect(ExperimentService.getInterim("p", "x")).resolves.toEqual({
      kind: "not_in_progress",
      state: "reported",
      error: "the run is not live"
    });
    const config = get.mock.calls[0][1] as { validateStatus: (s: number) => boolean };
    expect(config.validateStatus(409)).toBe(true);
    expect(config.validateStatus(404)).toBe(false);
  });

  it("returns a 200 body as the interim", async () => {
    const body = { experiment: "x", running: false, reading: null };
    get.mockResolvedValue({ status: 200, data: body });
    await expect(ExperimentService.getInterim("p", "x")).resolves.toEqual({
      kind: "interim",
      interim: body
    });
    expect(get.mock.calls[0][0]).toContain("/x/interim");
  });

  it("throws on any other 409 body", async () => {
    get.mockResolvedValue({ status: 409, data: "something else" });
    await expect(ExperimentService.getInterim("p", "x")).rejects.toThrow("something else");
  });

  it("requestInterim posts with no body and rethrows a 403 message", async () => {
    post.mockResolvedValue({ status: 202, data: { running: true } });
    await expect(ExperimentService.requestInterim("p", "x")).resolves.toEqual({ running: true });
    expect(post.mock.calls.at(-1)?.[0]).toContain("/x/interim");

    post.mockRejectedValue(failure(403, "you need edit rights"));
    await expect(ExperimentService.requestInterim("p", "x")).rejects.toThrow(
      "you need edit rights"
    );
  });
});
