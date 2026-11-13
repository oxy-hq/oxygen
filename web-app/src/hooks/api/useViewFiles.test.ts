// @vitest-environment jsdom
import { renderHook } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import useViewFiles from "./useViewFiles";

vi.mock("./useSemanticObjects", () => ({
  default: () => ({
    data: {
      views: [{ name: "orders", path: "semantics/views/orders.view.yml" }],
      topics: [{ name: "sales", path: "semantics/topics/sales.topic.yml" }]
    },
    isLoading: false,
    error: null
  })
}));

describe("useViewFiles", () => {
  it("lists only the views", () => {
    const { result } = renderHook(() => useViewFiles());
    expect(result.current.viewFiles).toHaveLength(1);
    expect(result.current.viewFiles[0]).toMatchObject({
      value: "orders",
      label: "orders",
      path: "semantics/views/orders.view.yml"
    });
  });
});
