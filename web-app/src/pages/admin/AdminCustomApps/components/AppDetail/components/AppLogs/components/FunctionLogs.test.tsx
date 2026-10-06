// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useAppLogs } from "@/hooks/api/customApps/useCustomApps";
import type { FunctionLogLine } from "@/types/apps";
import { REQUEST_LOG_LIMIT, WINDOW_LOG_LIMIT } from "../functionLogs";
import { FunctionLogs } from "./FunctionLogs";

vi.mock("@/hooks/api/customApps/useCustomApps", () => ({ useAppLogs: vi.fn() }));

type LogsQuery = ReturnType<typeof useAppLogs>;

const REQUEST = "0b7a1c2e-5d3f-4a6b-8c9d-0e1f2a3b4c5d";

function line(overrides: Partial<FunctionLogLine>): FunctionLogLine {
  return {
    timestamp: "2026-10-03T14:02:11.000000Z",
    build_id: "build-1",
    invocation_id: "inv-a",
    request_id: REQUEST,
    function_name: "syncOrders",
    mode: "route",
    level: "info",
    seq: 0,
    message: "",
    trace_id: "9f3e2d1c0b0a49f8a7b6c5d4e3f2a1b0",
    environment: "production",
    ...overrides
  };
}

const answerWith = (answer: Partial<LogsQuery>) =>
  vi
    .mocked(useAppLogs)
    .mockReturnValue({ data: [], isLoading: false, error: null, ...answer } as LogsQuery);

/** What the hook was last asked for. */
const lastQuery = () => {
  const calls = vi.mocked(useAppLogs).mock.calls;
  return calls[calls.length - 1][2];
};

const mount = (hours: 1 | 24 | 168 = 24) =>
  render(<FunctionLogs orgSlug='acme' appSlug='pos' hours={hours} />);

const typeRequest = (value: string) =>
  fireEvent.change(screen.getByTestId("admin-app-logs-request-filter"), { target: { value } });

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("FunctionLogs", () => {
  it("reads the window it was given and shows one block per invocation", () => {
    answerWith({
      data: [
        line({ invocation_id: "inv-b", seq: 0, message: "b only" }),
        line({ invocation_id: "inv-a", seq: 1, message: "a second" }),
        line({ invocation_id: "inv-a", seq: 0, message: "a first" })
      ]
    });
    mount(24);

    expect(lastQuery()).toEqual({ hours: 24, limit: WINDOW_LOG_LIMIT });
    expect(screen.getAllByTestId("admin-app-logs-invocation")).toHaveLength(2);
    // The ids that were in the response and on screen nowhere.
    expect(screen.getAllByTestId("admin-app-logs-request-id")[0]).toHaveTextContent("0b7a1c2e");
    expect(screen.getAllByTestId("admin-app-logs-trace-id")[0]).toHaveTextContent("9f3e2d1c");
    expect(screen.queryByTestId("admin-app-logs-truncated")).not.toBeInTheDocument();
  });

  it("leaves out an id the line does not have", () => {
    answerWith({ data: [line({ trace_id: "", request_id: "" })] });
    mount();

    expect(screen.queryByTestId("admin-app-logs-trace-id")).not.toBeInTheDocument();
    expect(screen.queryByTestId("admin-app-logs-request-id")).not.toBeInTheDocument();
  });

  it("searches the widest window for a pasted request id, whatever the picker says", () => {
    answerWith({ data: [] });
    mount(1);

    typeRequest(`  ${REQUEST.toUpperCase()}  `);

    expect(lastQuery()).toEqual({ hours: 168, limit: REQUEST_LOG_LIMIT, requestId: REQUEST });
    expect(screen.getByTestId("admin-app-logs-empty")).toHaveTextContent(
      "No function output for this request in the last 7 days."
    );
  });

  it("holds a partial id back instead of sending it", () => {
    // The route answers a non-UUID with a 400, which would render as "could
    // not read function logs" while the operator is still typing.
    answerWith({ data: [line({ message: "unfiltered" })] });
    mount();

    typeRequest(REQUEST.slice(0, 13));

    expect(screen.getByTestId("admin-app-logs-filter-invalid")).toBeInTheDocument();
    // Nothing unfiltered is shown under a filter that is not one yet…
    expect(screen.queryByTestId("admin-app-logs-list")).not.toBeInTheDocument();
    // …and the partial id never reached the query.
    for (const [, , query] of vi.mocked(useAppLogs).mock.calls) {
      expect(query.requestId).toBeUndefined();
    }
  });

  it("says so when the page is full", () => {
    answerWith({
      data: Array.from({ length: WINDOW_LOG_LIMIT }, (_, seq) => line({ seq }))
    });
    mount(24);

    const notice = screen.getByTestId("admin-app-logs-truncated");
    expect(notice).toHaveTextContent(
      `Showing the newest ${WINDOW_LOG_LIMIT} lines of the last 24 hours.`
    );
    // The way to older output is the request filter. A narrower window is not:
    // the route answers with the newest lines, so it would show these again.
    expect(notice).toHaveTextContent("Filter by request id");
    expect(notice).not.toHaveTextContent(/narrow the window/i);
  });

  it("names the window in the empty state", () => {
    answerWith({ data: [] });
    mount(168);

    expect(screen.getByTestId("admin-app-logs-empty")).toHaveTextContent(
      "No function output in the last 7 days."
    );
  });

  it("reports a failed read as a failure, not as an empty log", () => {
    answerWith({ data: undefined, error: new Error("log query failed") });
    mount();

    expect(screen.getByTestId("admin-app-logs-error")).toBeInTheDocument();
    expect(screen.queryByTestId("admin-app-logs-empty")).not.toBeInTheDocument();
  });
});
