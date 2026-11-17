// @vitest-environment jsdom
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { AxiosError, AxiosHeaders } from "axios";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AppDataRows, AppDataTables } from "@/types/appData";
import type { CustomApp } from "@/types/apps";

vi.mock("@/services/api/appData", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/services/api/appData")>();
  return {
    ...actual,
    AppDataService: { listTables: vi.fn(), listRows: vi.fn() }
  };
});

import { AppDataError, AppDataService, toAppDataError } from "@/services/api/appData";
import { AppDataPanel } from "./index";

const app = { id: "app-1", name: "Store Ops", org_slug: "acme", slug: "store-ops" } as CustomApp;

const caps = { max_rows: 200, text_chars: 256, bytes_shown: 32, statement_timeout_ms: 5000 };

const ready: AppDataTables = {
  environment: "production",
  store: { state: "ready", schema: "app_store_ops" },
  tables: [
    {
      name: "orders",
      approx_rows: 1200,
      columns: [
        {
          name: "id",
          type: "uuid",
          kind: "other",
          nullable: false,
          primary_key: true,
          sortable: true
        },
        {
          name: "note",
          type: "text",
          kind: "text",
          nullable: true,
          primary_key: false,
          sortable: true
        },
        {
          name: "payload",
          type: "json",
          kind: "json",
          nullable: true,
          primary_key: false,
          sortable: false
        }
      ]
    },
    { name: "stores", approx_rows: null, columns: [] }
  ],
  caps
};

const page: AppDataRows = {
  environment: "production",
  schema: "app_store_ops",
  table: "orders",
  columns: ready.tables[0].columns,
  rows: [
    [{ text: "4c1d" }, null, { json: { ok: true } }],
    [{ text: "9a2e" }, { text: "NULL" }, null]
  ],
  limit: 50,
  requested_limit: 50,
  offset: 0,
  order: [{ column: "id", dir: "asc" }],
  has_more: true,
  caps
};

const renderPanel = (environment = "production", label = "Production") =>
  render(
    <QueryClientProvider client={new QueryClient()}>
      <AppDataPanel
        app={app}
        environment={environment}
        scope={{ label, warn: environment !== "production" }}
      />
    </QueryClientProvider>
  );

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe("AppDataPanel", () => {
  it("says it is read-only and reads nothing until asked", () => {
    renderPanel("staging", "Staging");
    expect(screen.getByTestId("admin-app-data-mode").textContent).toContain("Read-only");
    expect(screen.getByTestId("apps-console-panel-data-scope").textContent).toBe("Staging");
    expect(screen.getByTestId("admin-app-data-gate")).toBeTruthy();
    expect(AppDataService.listTables).not.toHaveBeenCalled();
  });

  it("lists the tables, then a page of one with NULL drawn apart from text", async () => {
    vi.mocked(AppDataService.listTables).mockResolvedValue(ready);
    vi.mocked(AppDataService.listRows).mockResolvedValue(page);
    renderPanel();
    fireEvent.click(screen.getByTestId("admin-app-data-browse"));

    expect(await screen.findByTestId("admin-app-data-schema")).toHaveProperty(
      "textContent",
      "app_store_ops"
    );
    expect(AppDataService.listTables).toHaveBeenCalledWith("app-1", "production");
    expect(screen.getByTestId("admin-app-data-table-stores").textContent).toContain("not analyzed");

    fireEvent.click(screen.getByTestId("admin-app-data-table-orders"));
    const grid = await screen.findByTestId("admin-app-data-grid");
    expect(AppDataService.listRows).toHaveBeenCalledWith("app-1", "production", {
      table: "orders",
      limit: 50,
      offset: 0,
      order: undefined
    });
    expect(grid.querySelector("thead")?.className).toContain("sticky");
    const cells = within(grid).getAllByRole("cell");
    const nulls = cells.filter((c) => c.dataset.null === "true");
    expect(nulls).toHaveLength(2);
    expect(nulls[0].className).toContain("italic");
    // The string "NULL" is a value, in monospace like every other.
    const text = cells.filter((c) => c.textContent === "NULL" && c.dataset.null === undefined);
    expect(text).toHaveLength(1);
    expect(text[0].className).toContain("font-mono");
    expect(screen.getByTestId("admin-app-data-range").textContent).toBe("Rows 1–2");
  });

  it("orders by a column the header names and pages on", async () => {
    vi.mocked(AppDataService.listTables).mockResolvedValue(ready);
    vi.mocked(AppDataService.listRows).mockResolvedValue(page);
    renderPanel();
    fireEvent.click(screen.getByTestId("admin-app-data-browse"));
    fireEvent.click(await screen.findByTestId("admin-app-data-table-orders"));
    fireEvent.click(await screen.findByTestId("admin-app-data-col-note"));
    await waitFor(() =>
      expect(AppDataService.listRows).toHaveBeenLastCalledWith("app-1", "production", {
        table: "orders",
        limit: 50,
        offset: 0,
        order: { column: "note", dir: "asc" }
      })
    );
    await waitFor(() =>
      expect(screen.getByTestId("admin-app-data-next")).toHaveProperty("disabled", false)
    );
    fireEvent.click(screen.getByTestId("admin-app-data-next"));
    await waitFor(() =>
      expect(AppDataService.listRows).toHaveBeenLastCalledWith(
        "app-1",
        "production",
        expect.objectContaining({ offset: 50 })
      )
    );
  });

  it("offers no sort on a column Postgres cannot order", async () => {
    vi.mocked(AppDataService.listTables).mockResolvedValue(ready);
    vi.mocked(AppDataService.listRows).mockResolvedValue(page);
    renderPanel();
    fireEvent.click(screen.getByTestId("admin-app-data-browse"));
    fireEvent.click(await screen.findByTestId("admin-app-data-table-orders"));
    await screen.findByTestId("admin-app-data-grid");

    const json = screen.getByTestId("admin-app-data-col-payload");
    expect(json.tagName).not.toBe("BUTTON");
    expect(json.closest("button")).toBeNull();
    expect(json.getAttribute("title")).toContain("cannot order");
    expect(screen.getByTestId("admin-app-data-col-note").tagName).toBe("BUTTON");

    const calls = vi.mocked(AppDataService.listRows).mock.calls.length;
    fireEvent.click(json);
    expect(vi.mocked(AppDataService.listRows).mock.calls.length).toBe(calls);
    expect(
      vi
        .mocked(AppDataService.listRows)
        .mock.calls.some(([, , asked]) => asked.order?.column === "payload")
    ).toBe(false);
  });

  it("offers the way back when a sorted page times out", async () => {
    vi.mocked(AppDataService.listTables).mockResolvedValue(ready);
    vi.mocked(AppDataService.listRows).mockImplementation(async (_id, _env, asked) => {
      if (asked.order) {
        throw new AppDataError("the read took longer than its time limit", "read_timeout", 504);
      }
      return page;
    });
    renderPanel();
    fireEvent.click(screen.getByTestId("admin-app-data-browse"));
    fireEvent.click(await screen.findByTestId("admin-app-data-table-orders"));
    await screen.findByTestId("admin-app-data-grid");
    expect(screen.queryByTestId("admin-app-data-reset")).toBeNull();

    fireEvent.click(screen.getByTestId("admin-app-data-col-note"));
    expect(await screen.findByText(/longer than its time limit/)).toBeTruthy();
    fireEvent.click(screen.getByTestId("admin-app-data-reset"));
    // The default page again (from cache: it was read a moment ago), headers and all.
    await screen.findByTestId("admin-app-data-grid");
    expect(screen.queryByText(/longer than its time limit/)).toBeNull();
    expect(screen.queryByTestId("admin-app-data-reset")).toBeNull();
    expect(screen.getByTestId("admin-app-data-col-note").closest("th")?.ariaSort).toBeFalsy();
  });

  it("explains an environment with no store of its own", async () => {
    vi.mocked(AppDataService.listTables).mockResolvedValue({
      environment: "staging",
      store: {
        state: "absent",
        code: "no_branch",
        reason: "staging has no OLTP store of its own: the org has no staging branch"
      },
      tables: [],
      caps
    });
    renderPanel("staging", "Staging");
    fireEvent.click(screen.getByTestId("admin-app-data-browse"));
    const empty = await screen.findByTestId("admin-app-data-absent");
    expect(empty.textContent).toContain("No OLTP data to browse in Staging.");
    expect(empty.textContent).toContain("the org has no staging branch.");
    expect(AppDataService.listRows).not.toHaveBeenCalled();
  });

  it("shows a refusal in the server's words", async () => {
    vi.mocked(AppDataService.listTables).mockRejectedValue(
      new AppDataError(
        "this app's org has locked Oxy staff out of the workspace the app was published from",
        "oxy_locked_out",
        403
      )
    );
    renderPanel();
    fireEvent.click(screen.getByTestId("admin-app-data-browse"));
    expect(await screen.findByText(/locked Oxy staff out/)).toBeTruthy();
  });
});

describe("toAppDataError", () => {
  const axiosError = (status: number, data: unknown) => {
    const config = { headers: new AxiosHeaders() };
    return new AxiosError("Request failed with status code 403", String(status), config, null, {
      status,
      statusText: "",
      headers: {},
      config,
      data
    });
  };

  it("carries the server's message and code, not Axios's", () => {
    const err = toAppDataError(
      axiosError(403, {
        error: "app_admin_required",
        message: "an app's data is open to the app's admins"
      })
    );
    expect(err).toBeInstanceOf(AppDataError);
    expect((err as AppDataError).message).toBe("an app's data is open to the app's admins");
    expect((err as AppDataError).code).toBe("app_admin_required");
    expect((err as AppDataError).status).toBe(403);
  });

  it("leaves an error with no message as it was", () => {
    const bare = axiosError(500, "");
    expect(toAppDataError(bare)).toBe(bare);
    const plain = new Error("offline");
    expect(toAppDataError(plain)).toBe(plain);
  });
});
