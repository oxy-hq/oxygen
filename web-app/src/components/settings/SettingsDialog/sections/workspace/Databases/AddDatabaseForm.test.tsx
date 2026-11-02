// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AddDatabaseForm } from "./AddDatabaseForm";

/**
 * "Create database connection" offers nothing that resolves against a disk in
 * a cloud deployment (`libs/utils/cloudConnections`): no file DuckDB, and key
 * fields that name a secret. Local mode keeps every option, DuckDB marked
 * deprecated.
 */

let isLocalMode = true;
vi.mock("@/contexts/AuthContext", () => ({ useAuth: () => ({ isLocalMode }) }));
vi.mock("@/hooks/api/databases/useCreateDatabaseConfig", () => ({
  useCreateDatabaseConfig: () => ({ mutateAsync: vi.fn(), isPending: false })
}));
vi.mock("@/hooks/api/databases/useTestDatabaseConnection", () => ({
  useTestDatabaseConnection: () => ({ testConnection: vi.fn(), reset: vi.fn(), isLoading: false })
}));
// Not under test: the test-connection panel and Radix's select, which jsdom cannot open.
vi.mock("./TestConnectionSection", () => ({ TestConnectionSection: () => null }));
vi.mock("@/components/ui/shadcn/select", () => ({
  Select: ({
    value,
    onValueChange,
    children
  }: {
    value: string;
    onValueChange: (value: string) => void;
    children: React.ReactNode;
  }) => (
    <select aria-label='Auth Mode' value={value} onChange={(e) => onValueChange(e.target.value)}>
      {children}
    </select>
  ),
  SelectTrigger: () => null,
  SelectValue: () => null,
  SelectContent: ({ children }: { children: React.ReactNode }) => <>{children}</>,
  SelectItem: ({ value, children }: { value: string; children: React.ReactNode }) => (
    <option value={value}>{children}</option>
  )
}));

function renderForm() {
  return render(<AddDatabaseForm onSuccess={() => {}} onCancel={() => {}} />);
}

beforeEach(() => {
  isLocalMode = true;
});
afterEach(cleanup);

describe("AddDatabaseForm in local mode", () => {
  it("offers DuckDB, marked deprecated with a pointer to Airhouse", () => {
    renderForm();
    fireEvent.click(screen.getByRole("button", { name: "DuckDB (deprecated)" }));
    expect(screen.getByRole("note").textContent).toMatch(/deprecated.*Use Airhouse/);
    expect(screen.getByPlaceholderText("/path/to/data")).toBeTruthy();
  });

  it("keeps the BigQuery key textarea and the Snowflake key path", () => {
    renderForm();
    expect(screen.getByText("Service Account Key (JSON)")).toBeTruthy();
    expect(screen.queryByText("Service Account Key Secret")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Snowflake" }));
    fireEvent.change(screen.getByLabelText("Auth Mode"), { target: { value: "private_key" } });
    expect(screen.getByPlaceholderText("/path/to/rsa_key.p8")).toBeTruthy();
  });
});

describe("AddDatabaseForm in a cloud deployment", () => {
  beforeEach(() => {
    isLocalMode = false;
  });

  it("does not offer DuckDB", () => {
    renderForm();
    expect(screen.queryByRole("button", { name: /DuckDB/ })).toBeNull();
    for (const name of ["BigQuery", "Snowflake", "PostgreSQL", "ClickHouse"]) {
      expect(screen.getByRole("button", { name })).toBeTruthy();
    }
  });

  it("asks for the secret holding the BigQuery key, not the key", () => {
    renderForm();
    expect(screen.getByLabelText("Service Account Key Secret")).toBeTruthy();
    expect(screen.queryByText("Service Account Key (JSON)")).toBeNull();
  });

  it("asks for the secret holding the Snowflake private key, not a path", () => {
    renderForm();
    fireEvent.click(screen.getByRole("button", { name: "Snowflake" }));
    fireEvent.change(screen.getByLabelText("Auth Mode"), { target: { value: "private_key" } });
    expect(screen.getByLabelText("Private Key Secret")).toBeTruthy();
    expect(screen.queryByPlaceholderText("/path/to/rsa_key.p8")).toBeNull();
  });

  it("keeps password and browser auth for Snowflake", () => {
    renderForm();
    fireEvent.click(screen.getByRole("button", { name: "Snowflake" }));
    const modes = Array.from(
      (screen.getByLabelText("Auth Mode") as HTMLSelectElement).options,
      (o) => o.value
    );
    expect(modes).toEqual(["password", "browser", "private_key"]);
  });
});
