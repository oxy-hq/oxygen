// @vitest-environment jsdom

import { describe, expect, it } from "vitest";
import type { HomeWidgetsResponse } from "@/types/homeWidgets";
import { refreshEvery } from "./useHomeWidgets";

const feed = (asks: Array<number | null>): HomeWidgetsResponse =>
  ({
    sections: [],
    widgets: asks.map((refresh_after) => ({
      view: refresh_after === null ? null : { refresh_after }
    }))
  }) as unknown as HomeWidgetsResponse;

describe("refreshEvery", () => {
  it("polls on the soonest widget's refresh_after, in milliseconds", () => {
    expect(refreshEvery(feed([600, 120, null]))).toBe(120_000);
  });

  it("does not poll when no widget asks", () => {
    expect(refreshEvery(feed([null]))).toBe(false);
    expect(refreshEvery(undefined)).toBe(false);
  });
});
