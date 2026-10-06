// @vitest-environment jsdom

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { useAppClientErrors } from "@/hooks/api/customApps/useCustomApps";
import type { ClientError } from "@/types/apps";
import { ClientErrors } from "./ClientErrors";

vi.mock("@/hooks/api/customApps/useCustomApps", () => ({ useAppClientErrors: vi.fn() }));

type ErrorsQuery = ReturnType<typeof useAppClientErrors>;

function fault(overrides: Partial<ClientError> = {}): ClientError {
  return {
    stack_hash: "h1",
    error_name: "TypeError",
    message: "Cannot read properties of undefined (reading 'map')",
    stack:
      "TypeError: Cannot read properties of undefined\n    at OrderBoard (OrderBoard.tsx:142:31)",
    stack_resolved: true,
    build_id: "b1",
    path: "/orders/today",
    kind: "error",
    occurrences: 2140,
    sessions: 318,
    first_seen: "2026-10-01T08:00:00Z",
    last_seen: "2026-10-06T13:58:40Z",
    ...overrides
  };
}

const answerWith = (answer: Partial<ErrorsQuery>) =>
  vi
    .mocked(useAppClientErrors)
    .mockReturnValue({ data: [], isLoading: false, error: null, ...answer } as ErrorsQuery);

const mount = () => render(<ClientErrors orgSlug='acme' appSlug='pos' hours={24} />);

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("ClientErrors", () => {
  it("gives each count a cell of its own and keeps the stack for the opened row", () => {
    answerWith({ data: [fault()] });
    mount();
    const row = screen.getByTestId("admin-app-errors-group-h1");

    // Two figures, not the sentence "2,140× across 318 sessions".
    expect(row).toHaveTextContent("2,140");
    expect(row).toHaveTextContent("318");
    expect(row).not.toHaveTextContent("across");
    expect(row).not.toHaveTextContent("OrderBoard.tsx");

    const toggle = row.querySelector("button");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(toggle as HTMLButtonElement);

    expect(toggle).toHaveAttribute("aria-expanded", "true");
    expect(row).toHaveTextContent("OrderBoard.tsx:142:31");
    expect(row).toHaveTextContent("/orders/today");
    expect(row).toHaveTextContent("2026-10-06T13:58:40Z");
    expect(row).not.toHaveTextContent("unhandled rejection");
    expect(screen.queryByTestId("admin-app-errors-unresolved")).not.toBeInTheDocument();
  });

  // A still-minified stack looks like a resolved one until someone tries to
  // open the file. The note has to be there exactly when that is true.
  it("says a stack is unresolved, and what kind of fault an unhandled rejection is", () => {
    answerWith({ data: [fault({ stack_resolved: false, kind: "unhandledrejection" })] });
    mount();
    fireEvent.click(
      screen.getByTestId("admin-app-errors-group-h1").querySelector("button") as HTMLButtonElement
    );

    expect(screen.getByTestId("admin-app-errors-unresolved")).toHaveTextContent(
      "no source map was published"
    );
    expect(screen.getByTestId("admin-app-errors-group-h1")).toHaveTextContent(
      "unhandled rejection"
    );
  });

  it("tells a failed read from a quiet app", () => {
    answerWith({ data: undefined, error: new Error("errors query failed") });
    const { unmount } = mount();
    expect(screen.getByTestId("admin-app-errors-error")).toBeInTheDocument();
    expect(screen.queryByTestId("admin-app-errors-empty")).not.toBeInTheDocument();
    unmount();

    answerWith({ data: [] });
    mount();
    expect(screen.getByTestId("admin-app-errors-empty")).toHaveTextContent(
      "No uncaught browser errors"
    );
  });
});
