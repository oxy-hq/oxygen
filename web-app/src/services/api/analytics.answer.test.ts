// @vitest-environment jsdom

import type { AxiosAdapter } from "axios";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { AnalyticsService } from "./analytics";
import { apiClient } from "./axios";

/**
 * A Builder run is answered on the Builder route; the chat route refuses one with
 * 421 on every server. A caller that knows goes straight there (one round trip, which
 * a page navigating away cannot cut in half); one that does not is sent on by the 421.
 */

let posted: string[] = [];
const original = apiClient.defaults.adapter;

beforeEach(() => {
  posted = [];
  const adapter: AxiosAdapter = async (config) => {
    const url = config.url ?? "";
    posted.push(url);
    if (url.includes("/analytics/runs/") && url.endsWith("/answer")) {
      const error = Object.assign(new Error("misdirected"), {
        isAxiosError: true,
        response: { status: 421, data: {}, headers: {}, config, statusText: "" },
        config
      });
      throw error;
    }
    return {
      data: { ok: true, resumed: true },
      status: 200,
      statusText: "OK",
      headers: {},
      config
    };
  };
  apiClient.defaults.adapter = adapter;
});
afterEach(() => {
  apiClient.defaults.adapter = original;
});

describe("AnalyticsService.submitAnswer", () => {
  it("sends a Builder run's answer straight to the Builder route", async () => {
    await AnalyticsService.submitAnswer("ws-1", "run-1", "Accept", { builder: true });
    expect(posted).toEqual(["/ws-1/analytics/builder/runs/run-1/answer"]);
  });

  it("follows the chat route's 421 to the Builder route when it does not know", async () => {
    await AnalyticsService.submitAnswer("ws-1", "run-1", "Accept");
    expect(posted).toEqual([
      "/ws-1/analytics/runs/run-1/answer",
      "/ws-1/analytics/builder/runs/run-1/answer"
    ]);
  });
});

describe("AnalyticsService.submitAnswer while another server drives the run", () => {
  const unavailable = (config: Parameters<AxiosAdapter>[0], retryable: boolean) =>
    Object.assign(new Error("unavailable"), {
      isAxiosError: true,
      response: {
        status: 503,
        data: retryable ? { error: "busy", retryable: true } : { error: "down" },
        headers: retryable ? { "retry-after": "0" } : {},
        config,
        statusText: ""
      },
      config
    });

  const answering = (failures: number, retryable: boolean): AxiosAdapter => {
    let left = failures;
    return async (config) => {
      posted.push(config.url ?? "");
      if (left > 0) {
        left -= 1;
        throw unavailable(config, retryable);
      }
      return {
        data: { ok: true, resumed: true },
        status: 200,
        statusText: "OK",
        headers: {},
        config
      };
    };
  };

  it("retries a retryable 503 until the answer goes through", async () => {
    apiClient.defaults.adapter = answering(2, true);
    await expect(AnalyticsService.submitAnswer("ws-1", "run-1", "yes")).resolves.toEqual({
      ok: true,
      resumed: true
    });
    expect(posted).toEqual([
      "/ws-1/analytics/runs/run-1/answer",
      "/ws-1/analytics/runs/run-1/answer",
      "/ws-1/analytics/runs/run-1/answer"
    ]);
  });

  it("does not retry a 503 that is not marked retryable", async () => {
    apiClient.defaults.adapter = answering(1, false);
    await expect(AnalyticsService.submitAnswer("ws-1", "run-1", "yes")).rejects.toThrow();
    expect(posted).toHaveLength(1);
  });

  it("does not retry a 503 that has Retry-After but no retryable mark", async () => {
    apiClient.defaults.adapter = async (config) => {
      posted.push(config.url ?? "");
      throw Object.assign(new Error("unavailable"), {
        isAxiosError: true,
        response: {
          status: 503,
          data: { error: "gateway" },
          headers: { "retry-after": "0" },
          config,
          statusText: ""
        },
        config
      });
    };
    await expect(AnalyticsService.submitAnswer("ws-1", "run-1", "yes")).rejects.toThrow();
    expect(posted).toHaveLength(1);
  });

  it("gives up after a bounded number of tries", async () => {
    apiClient.defaults.adapter = answering(100, true);
    await expect(AnalyticsService.submitAnswer("ws-1", "run-1", "yes")).rejects.toThrow();
    expect(posted).toHaveLength(15);
  });

  it("stops retrying once the caller aborts", async () => {
    const controller = new AbortController();
    controller.abort();
    apiClient.defaults.adapter = answering(5, true);
    await expect(
      AnalyticsService.submitAnswer("ws-1", "run-1", "yes", { signal: controller.signal })
    ).rejects.toThrow();
    expect(posted).toHaveLength(1);
  });
});
