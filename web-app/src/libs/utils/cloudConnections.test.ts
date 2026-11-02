import { describe, expect, it } from "vitest";
import { bigQueryKeySecretName, offeredDatabaseTypes } from "./cloudConnections";

const OPTIONS = [
  { type: "bigquery", label: "BigQuery" },
  { type: "duckdb", label: "DuckDB" },
  { type: "snowflake", label: "Snowflake" }
];

describe("offeredDatabaseTypes", () => {
  it("offers every type in local mode, in order", () => {
    expect(offeredDatabaseTypes(OPTIONS, true).map((o) => o.type)).toEqual([
      "bigquery",
      "duckdb",
      "snowflake"
    ]);
  });

  it("offers no file DuckDB in a cloud deployment", () => {
    expect(offeredDatabaseTypes(OPTIONS, false).map((o) => o.type)).toEqual([
      "bigquery",
      "snowflake"
    ]);
  });

  it("does not modify the list it is given", () => {
    offeredDatabaseTypes(OPTIONS, false);
    expect(OPTIONS).toHaveLength(3);
  });
});

describe("bigQueryKeySecretName", () => {
  it("names the key's secret after the connection, as a pasted password is", () => {
    expect(bigQueryKeySecretName("bigquery")).toBe("BIGQUERY_KEY_JSON");
    expect(bigQueryKeySecretName("Sales_BQ")).toBe("SALES_BQ_KEY_JSON");
  });
});
