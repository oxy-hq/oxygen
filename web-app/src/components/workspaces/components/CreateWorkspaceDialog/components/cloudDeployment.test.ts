// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import {
  deriveMessages,
  deriveRailState,
  initialState,
  warehouseTypeOptions
} from "./orchestrator";
import type { OnboardingState } from "./types";
import { buildWarehouseConfig } from "./useOnboardingActions";

/**
 * In a cloud deployment the wizard creates no file DuckDB and no key file
 * (`libs/utils/cloudConnections`); local mode keeps DuckDB, marked deprecated.
 */

const credentialsStep = (warehouseType: OnboardingState["warehouseType"]): OnboardingState => ({
  ...initialState,
  mode: "new",
  step: "warehouse_credentials",
  llmProvider: "anthropic",
  llmModel: "claude",
  llmApiKey: "k",
  warehouseType
});

function credentialFields(state: OnboardingState, cloud: boolean) {
  const block = deriveMessages(state, { cloud }).find(
    (m) => m.id === "warehouse_credentials"
  )?.inputBlock;
  if (block?.type !== "credential_form") throw new Error("no credential form");
  return block.fields;
}

describe("warehouse choices", () => {
  it("offers DuckDB in local mode, marked deprecated", () => {
    const duckdb = warehouseTypeOptions({ cloud: false }).find((o) => o.id === "duckdb");
    expect(duckdb?.label).toBe("DuckDB");
    expect(duckdb?.description).toMatch(/Deprecated/);
  });

  it("offers an upload into Airhouse, not DuckDB, in a cloud deployment", () => {
    const options = warehouseTypeOptions({ cloud: true });
    expect(options.map((o) => o.label)).not.toContain("DuckDB");
    expect(options.find((o) => o.id === "duckdb")).toMatchObject({
      label: "Upload files",
      description: "CSV or Parquet, loaded into Airhouse"
    });
  });

  it("says the upload lands in Airhouse, and never that someone has to set it up", () => {
    const fields = credentialFields(credentialsStep("duckdb"), true);
    const upload = fields.find((f) => f.type === "file_upload");
    expect(upload?.helperText).toMatch(/loaded into your Airhouse database/);
    expect(upload?.helperText).not.toMatch(/\.db\//);
    const text = JSON.stringify(deriveMessages(credentialsStep("duckdb"), { cloud: true }));
    expect(text).not.toMatch(/engineer|Oxy team|ask an/i);
  });

  it("keeps the local upload copy as it was", () => {
    const upload = credentialFields(credentialsStep("duckdb"), false).find(
      (f) => f.type === "file_upload"
    );
    expect(upload?.helperText).toMatch(/uploaded to \.db\/ inside your workspace/);
  });

  it("names the uploaded connection Airhouse on the rail in a cloud deployment", () => {
    const state = { ...credentialsStep("duckdb"), connectionStatus: "success" as const };
    expect(deriveRailState(state, { cloud: true }).connectedServices).toContainEqual(
      expect.objectContaining({ type: "warehouse", name: "Airhouse" })
    );
    expect(deriveRailState(state, { cloud: false }).connectedServices).toContainEqual(
      expect.objectContaining({ type: "warehouse", name: "Duckdb" })
    );
  });
});

describe("buildWarehouseConfig", () => {
  it("creates a file DuckDB only in local mode", () => {
    expect(buildWarehouseConfig("duckdb", { dataset: ".db" })).toEqual({
      type: "duckdb",
      name: "duckdb",
      config: { file_search_path: ".db" }
    });
    expect(buildWarehouseConfig("duckdb", { dataset: ".db" }, { cloud: true })).toEqual({
      type: "airhouse_managed",
      name: "duckdb",
      config: {}
    });
  });

  it("names the BigQuery key's secret in a cloud deployment, never its contents", () => {
    const credentials = { key_json: '{"type":"service_account"}', dataset: "sales" };
    expect(buildWarehouseConfig("bigquery", credentials).config).toEqual({
      key: credentials.key_json,
      dataset: "sales"
    });
    const cloud = buildWarehouseConfig("bigquery", credentials, { cloud: true }).config;
    expect(cloud).toEqual({ key_json_var: "BIGQUERY_KEY_JSON", dataset: "sales" });
    expect(JSON.stringify(cloud)).not.toContain("service_account");
  });
});
