import type { AsyncDuckDBConnection } from "@duckdb/duckdb-wasm";
import type { DisplayFormat } from "@/types/app";
import {
  getArrowColumnKeys,
  getArrowColumnValues,
  getArrowFieldType,
  inferColumnFormat
} from "../utils";

/** Wraps an identifier in double quotes, escaping any embedded double quotes. */
const q = (identifier: string) => `"${identifier.replace(/"/g, '""')}"`;

/**
 * The format for a chart's value column: the one the app declares, else the one
 * the column's name and type imply. That is `inferColumnFormat`, the rule a
 * table applies to each of its columns, so a chart of `payment_count` gets no
 * more of a `$` on its axis than a table gives that column. The type is the
 * source column's, read without fetching a row.
 */
export const resolveValueFormat = async (
  connection: AsyncDuckDBConnection,
  fileName: string,
  valueField: string,
  declared?: DisplayFormat
): Promise<DisplayFormat | undefined> => {
  if (declared) return declared;
  const column = await connection.query(`SELECT ${q(valueField)} FROM "${fileName}" LIMIT 0`);
  return inferColumnFormat(valueField, getArrowFieldType(valueField, column.schema));
};

/**
 * The x axis's categories, in order: each one's label, and the key a series'
 * point is matched to it by. A key is the full value; a label only as precise
 * as the column needs, so two moments within a minute can share a label only
 * when they are equal, and are never one point.
 */
export type XAxisData = { keys: (string | number)[]; labels: (string | number)[] };

export const getXAxisData = async (
  connection: AsyncDuckDBConnection,
  fileName: string,
  xField: string
): Promise<XAxisData> => {
  const xData = await connection.query(
    `SELECT DISTINCT ${q(xField)} as x FROM "${fileName}" ORDER BY ${q(xField)}`
  );
  return {
    keys: getArrowColumnKeys(xData, "x") as (string | number)[],
    labels: getArrowColumnValues(xData, "x") as (string | number)[]
  };
};

/**
 * A series: its name, as the legend shows it, and the key its rows are selected
 * by (`getSeriesValues`). For a moment the key is its exact instant; the name is
 * the clock time in the column's zone, which can stand for two instants.
 */
export type SeriesData = { name: unknown; key: unknown };

export const getSeriesData = async (
  connection: AsyncDuckDBConnection,
  fileName: string,
  seriesField: string
): Promise<SeriesData[]> => {
  const seriesStmt = await connection.prepare(
    `SELECT DISTINCT ${q(seriesField)} as series FROM "${fileName}"`
  );
  const series = await seriesStmt.query();
  const keys = getArrowColumnKeys(series, "series");
  return getArrowColumnValues(series, "series").map((name, index) => ({
    name,
    key: keys[index]
  }));
};

export const getSeriesValues = async (
  connection: AsyncDuckDBConnection,
  fileName: string,
  xField: string,
  yField: string,
  seriesField: string,
  seriesKey: unknown
): Promise<{ x: number | string; y: number | string }[]> => {
  const seriesDataStatement = await connection.prepare(
    `SELECT ${q(xField)} as x, SUM(${q(yField)}) as y FROM "${fileName}"
     WHERE ${q(seriesField)} = ?
     GROUP BY ${q(xField)}, ${q(seriesField)}
     ORDER BY ${q(xField)}`
  );
  const result = await seriesDataStatement.query(seriesKey);
  // Keyed as the x axis is keyed (`getXAxisData`), to find each point's place on it.
  const xValues = getArrowColumnKeys(result, "x");
  const yValues = getArrowColumnValues(result, "y");
  return xValues.map((x: unknown, index: number) => ({
    x: x as number | string,
    y: yValues[index] as number | string
  }));
};

export const getSimpleAggregatedData = async (
  connection: AsyncDuckDBConnection,
  fileName: string,
  xField: string,
  yField: string
): Promise<(string | number)[]> => {
  const yData = await connection.query(
    `SELECT ${q(xField)} as x, SUM(${q(yField)}) as y FROM "${fileName}"
     GROUP BY ${q(xField)}
     ORDER BY ${q(xField)}`
  );
  return getArrowColumnValues(yData, "y") as (string | number)[];
};

export const getPieChartData = async (
  connection: AsyncDuckDBConnection,
  fileName: string,
  nameField: string,
  valueField: string
): Promise<{ name: string; value: number }[]> => {
  const pieData = await connection.query(
    `SELECT ${q(nameField)} as name, SUM(${q(valueField)}) as value
     FROM "${fileName}"
     GROUP BY ${q(nameField)}`
  );
  const nameValues = getArrowColumnValues(pieData, "name");
  const valueValues = getArrowColumnValues(pieData, "value");
  return nameValues.map((name: unknown, index: number) => ({
    name: name as string,
    value: valueValues[index] as number
  }));
};
