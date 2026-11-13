// @vitest-environment jsdom

import type { AxiosAdapter } from "axios";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { AnalyticsService } from "./analytics";
import { apiClient } from "./axios";

/**
 * The Builder went with the IDE, so nothing is sent to its deleted routes: a
 * Builder start and an answer to a Builder run go to the chat routes, which
 * refuse them with `410 builder_removed`.
 */

let posted: string[] = [];
const original = apiClient.defaults.adapter;

const gone = (config: Parameters<AxiosAdapter>[0]) =>
  Object.assign(new Error("gone"), {
    isAxiosError: true,
    response: {
      status: 410,
      data: { error: "builder_removed", message: "the Builder was removed with the IDE" },
      headers: {},
      config,
      statusText: ""
    },
    config
  });

beforeEach(() => {
  posted = [];
  apiClient.defaults.adapter = async (config) => {
    posted.push(config.url ?? "");
    throw gone(config);
  };
});
afterEach(() => {
  apiClient.defaults.adapter = original;
});

describe("Builder requests after the Builder's removal", () => {
  it("starts a Builder run on the chat route, which refuses it", async () => {
    await expect(
      AnalyticsService.createRun("ws-1", {
        agent_id: "__builder__",
        question: "q",
        domain: "builder"
      })
    ).rejects.toMatchObject({ response: { status: 410 } });
    expect(posted).toEqual(["/ws-1/analytics/runs"]);
  });

  it("answers on the chat route once, without a second Builder-route try", async () => {
    await expect(AnalyticsService.submitAnswer("ws-1", "run-1", "Accept")).rejects.toMatchObject({
      response: { status: 410 }
    });
    expect(posted).toEqual(["/ws-1/analytics/runs/run-1/answer"]);
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
