// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";

const get = vi.fn();
vi.mock("./axios", () => ({ apiClient: { get: (...args: unknown[]) => get(...args) } }));

import { MetricAnomaliesService } from "./metricAnomalies";

/**
 * `listMonitors` rebuilds the response field by field, so a field the server
 * sends and this function does not name never reaches the page.
 */
describe("MetricAnomaliesService.listMonitors", () => {
  beforeEach(() => get.mockReset());

  it("carries the file's notify block through to the page", async () => {
    const notify = { slack_channel: "C0123ABCDEF", min_severity: "medium" };
    get.mockResolvedValue({ data: { monitors: [], coverage: [], notify } });

    await expect(MetricAnomaliesService.listMonitors("p")).resolves.toEqual({
      monitors: [],
      coverage: [],
      notify
    });
    expect(get).toHaveBeenCalledWith("/p/semantic/monitors");
  });

  it("reads a response with no block, or from before coverage shipped, as empty", async () => {
    get.mockResolvedValue({ data: {} });

    await expect(MetricAnomaliesService.listMonitors("p")).resolves.toEqual({
      monitors: [],
      coverage: [],
      notify: null
    });
  });
});
