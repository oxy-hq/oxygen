// @vitest-environment jsdom

import { act, renderHook } from "@testing-library/react";
import type { ReactNode } from "react";
import { MemoryRouter, useLocation, useNavigate } from "react-router-dom";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import useTraces from "@/hooks/api/traces/useTraces";
import type { Trace } from "@/services/api/traces";
import { useTracesController } from "./useTracesController";

vi.mock("@/hooks/api/traces/useTraces", () => ({ default: vi.fn() }));

type TracesQuery = ReturnType<typeof useTraces>;

/** Every `useTraces` call answers with this page, whatever it asked for. */
const answerWith = (items: Trace[], total: number) =>
  vi
    .mocked(useTraces)
    .mockReturnValue({ data: { items, total }, isLoading: false } as unknown as TracesQuery);

const mount = (url: string) => {
  const wrapper = ({ children }: { children: ReactNode }) => (
    <MemoryRouter initialEntries={[url]}>{children}</MemoryRouter>
  );
  return renderHook(
    () => ({
      ctl: useTracesController({ enabled: true }),
      search: useLocation().search,
      navigate: useNavigate()
    }),
    { wrapper }
  );
};

/** The filters the paged list query was last asked for. */
const listRequest = () => {
  const paged = vi
    .mocked(useTraces)
    .mock.calls.map(([options]) => options)
    .filter((options) => options?.limit === 10);
  return paged[paged.length - 1];
};

const settleSearch = () =>
  act(() => {
    vi.advanceTimersByTime(300);
  });

beforeEach(() => {
  vi.useFakeTimers();
  // Enough traces that any page a test names exists; the clamp tests override it.
  answerWith([], 500);
});
afterEach(() => {
  vi.useRealTimers();
  vi.clearAllMocks();
});

describe("useTracesController", () => {
  it("opens on the view the URL names", () => {
    const { result } = mount("/traces?range=7d&q=revenue&status=error&page=3");

    expect(result.current.ctl.timeRange).toEqual({ kind: "preset", value: "7d" });
    expect(result.current.ctl.searchInput).toBe("revenue");
    expect(result.current.ctl.status).toBe("error");
    expect(result.current.ctl.currentPage).toBe(3);
    // …and asks the API for it, not for page 1 of everything.
    expect(listRequest()).toMatchObject({
      duration: "7d",
      search: "revenue",
      status: "Error",
      offset: 20
    });
  });

  it("writes a filter change to the URL and goes back to page 1", () => {
    const { result } = mount("/traces?page=3");

    act(() => result.current.ctl.setStatus("error"));

    expect(result.current.search).toBe("?status=error");
    expect(result.current.ctl.currentPage).toBe(1);
  });

  it("keeps the other filters when the page changes", () => {
    const { result } = mount("/traces?range=7d&status=error");

    act(() => result.current.ctl.handlePageChange(4));

    expect(result.current.search).toBe("?range=7d&status=error&page=4");
  });

  it("puts the search in the URL once typing pauses, not on every key", () => {
    const { result } = mount("/traces?page=2");

    act(() => result.current.ctl.setSearchInput("rev"));
    expect(result.current.search).toBe("?page=2");

    settleSearch();
    expect(result.current.search).toBe("?q=rev");
    expect(listRequest()).toMatchObject({ search: "rev", offset: 0 });
  });

  it("follows a search that changed underneath it instead of writing the old one back", () => {
    // The sidebar's own Traces link while a search is showing: the URL loses
    // `q` without this hook having written it. The box has to empty, and the
    // debounce must not notice "box says revenue, URL says nothing" and put
    // the old text straight back.
    const { result } = mount("/traces?q=revenue");

    act(() => {
      void result.current.navigate("/traces");
    });
    expect(result.current.ctl.searchInput).toBe("");

    settleSearch();
    expect(result.current.search).toBe("");
    expect(listRequest()).toMatchObject({ search: "" });
  });

  it("lands on the last real page when the URL names one past the end", () => {
    // A shared link outlives the result set it was copied from. Page 40 of 25
    // traces is an empty list under a pager that says there are three pages.
    answerWith([], 25);
    const { result } = mount("/traces?status=error&page=40");

    expect(result.current.search).toBe("?status=error&page=3");
    expect(result.current.ctl.currentPage).toBe(3);
  });

  it("does not clamp before the list has answered", () => {
    // Loading looks like "zero traces" to anything that only reads the total.
    // Clamping then would throw away the page a link asked for on every load.
    vi.mocked(useTraces).mockReturnValue({
      data: undefined,
      isLoading: true
    } as unknown as TracesQuery);
    const { result } = mount("/traces?page=40");

    expect(result.current.search).toBe("?page=40");
    expect(result.current.ctl.currentPage).toBe(40);
  });
});
