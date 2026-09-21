// @vitest-environment jsdom
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AdminAsync } from "./AdminAsync";

/**
 * The state machine, tested at the seam that used to be copy-pasted: which of the four
 * outcomes wins, and in what order. The precedence is the whole point — `isError` with
 * stale `data` still in hand must show the failure, and a query that resolved to
 * `undefined` must not reach the children (fifteen hand-written call sites each had to
 * remember `isError || !data`, and reaching `children` with no data is a crash).
 */

afterEach(cleanup);

const body = (d: { items: string[] }) => <p>{`loaded ${d.items.length}`}</p>;

describe("AdminAsync", () => {
  it("shows skeleton bars while loading, not the children", () => {
    render(
      <AdminAsync query={{ isPending: true, data: undefined }} noun='compiles' rows={4}>
        {body}
      </AdminAsync>
    );
    expect(screen.getByTestId("admin-async-loading")).toBeTruthy();
    expect(screen.queryByText(/loaded/)).toBeNull();
  });

  it("accepts isLoading for hooks that don't speak isPending", () => {
    render(
      <AdminAsync query={{ isLoading: true, data: undefined }} noun='compiles'>
        {body}
      </AdminAsync>
    );
    expect(screen.getByTestId("admin-async-loading")).toBeTruthy();
  });

  it("names the noun in the failure and offers a retry that calls refetch", async () => {
    const refetch = vi.fn();
    render(
      <AdminAsync query={{ isError: true, data: undefined, refetch }} noun='the audit log'>
        {body}
      </AdminAsync>
    );
    expect(screen.getByTestId("admin-async-error").textContent).toContain("load the audit log");
    screen.getByTestId("admin-async-retry").click();
    expect(refetch).toHaveBeenCalledTimes(1);
  });

  it("omits the retry when the caller has no refetch to offer", () => {
    render(
      <AdminAsync query={{ isError: true, data: undefined }} noun='compiles'>
        {body}
      </AdminAsync>
    );
    expect(screen.queryByTestId("admin-async-retry")).toBeNull();
  });

  it("treats resolved-but-undefined as a failure rather than rendering children", () => {
    render(
      <AdminAsync query={{ data: undefined }} noun='compiles'>
        {body}
      </AdminAsync>
    );
    expect(screen.getByTestId("admin-async-error")).toBeTruthy();
  });

  it("prefers the failure over stale data still in the cache", () => {
    render(
      <AdminAsync query={{ isError: true, data: { items: ["a"] } }} noun='compiles'>
        {body}
      </AdminAsync>
    );
    expect(screen.getByTestId("admin-async-error")).toBeTruthy();
    expect(screen.queryByText("loaded 1")).toBeNull();
  });

  it("shows the empty state only when the predicate says so", () => {
    const view = (items: string[]) =>
      render(
        <AdminAsync
          query={{ data: { items } }}
          noun='compiles'
          isEmpty={(d) => d.items.length === 0}
          empty={<p>nothing here</p>}
        >
          {body}
        </AdminAsync>
      );
    view([]);
    expect(screen.getByTestId("admin-async-empty").textContent).toBe("nothing here");
    cleanup();
    view(["a", "b"]);
    expect(screen.getByText("loaded 2")).toBeTruthy();
  });

  it("renders children for empty data when the caller supplied no empty state", () => {
    render(
      <AdminAsync query={{ data: { items: [] } }} noun='compiles' isEmpty={() => true}>
        {body}
      </AdminAsync>
    );
    expect(screen.getByText("loaded 0")).toBeTruthy();
  });

  it("hands the loaded data to the children", () => {
    render(
      <AdminAsync query={{ data: { items: ["a", "b", "c"] } }} noun='compiles'>
        {body}
      </AdminAsync>
    );
    expect(screen.getByText("loaded 3")).toBeTruthy();
  });
});

/**
 * The server's own words. Two pages showed `error.message` before this component existed
 * and lost it in the migration; that detail is the difference between "retry" and "go fix
 * a credential", so it is asserted rather than left to a reviewer's eye.
 */
describe("AdminAsync error detail", () => {
  const detail = () => screen.queryByTestId("admin-async-error-detail")?.textContent ?? null;

  it("shows what the server said", () => {
    render(
      <AdminAsync
        query={{ isError: true, data: undefined, error: new Error("AIRHOUSE_WIRE_HOST unset") }}
        noun='warehouses'
      >
        {body}
      </AdminAsync>
    );
    expect(detail()).toBe("AIRHOUSE_WIRE_HOST unset");
  });

  it("reads a message off a plain object or a bare string too", () => {
    render(
      <AdminAsync query={{ isError: true, data: undefined, error: "boom" }} noun='x'>
        {body}
      </AdminAsync>
    );
    expect(detail()).toBe("boom");
    cleanup();
    render(
      <AdminAsync query={{ isError: true, data: undefined, error: { message: "thud" } }} noun='x'>
        {body}
      </AdminAsync>
    );
    expect(detail()).toBe("thud");
  });

  it("suppresses axios's boilerplate, which tells the operator nothing", () => {
    render(
      <AdminAsync
        query={{
          isError: true,
          data: undefined,
          error: new Error("Request failed with status code 500")
        }}
        noun='x'
      >
        {body}
      </AdminAsync>
    );
    expect(detail()).toBeNull();
  });

  it("shows nothing extra when there is no error to quote", () => {
    render(
      <AdminAsync query={{ isError: true, data: undefined }} noun='x'>
        {body}
      </AdminAsync>
    );
    expect(detail()).toBeNull();
    cleanup();
    render(
      <AdminAsync query={{ isError: true, data: undefined, error: new Error("   ") }} noun='x'>
        {body}
      </AdminAsync>
    );
    expect(detail()).toBeNull();
  });

  it("truncates a message long enough to bury the page", () => {
    render(
      <AdminAsync
        query={{ isError: true, data: undefined, error: new Error("x".repeat(500)) }}
        noun='x'
      >
        {body}
      </AdminAsync>
    );
    expect(detail()).toHaveLength(301);
    expect(detail()?.endsWith("…")).toBe(true);
  });
});
