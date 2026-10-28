/** How a numeric value is formatted for display in charts + tables. */
export type DisplayFormat = "currency" | "percent" | "number";

export type LineChartDisplay = {
  type: "line_chart" | "line";
  x: string;
  y: string;
  xAxisTitle?: string;
  yAxisTitle?: string;
  data: string;
  series?: string;
  title?: string;
  /** Optional formatting for the y-axis + tooltip values. */
  y_format?: DisplayFormat;
};

export type BarChartDisplay = {
  type: "bar_chart" | "bar";
  data: string;
  x: string;
  y: string;
  title?: string;
  series?: string;
  /** Optional formatting for the y-axis + tooltip values. */
  y_format?: DisplayFormat;
};

export type PieChartDisplay = {
  type: "pie_chart" | "pie";
  data: string;
  name: string;
  value: string;
  title?: string;
  /** Optional formatting for the slice value in the tooltip. */
  value_format?: DisplayFormat;
};

export type TableDisplay = {
  type: "table";
  data: string;
  title?: string;
  /**
   * Optional per-column number formatting. Keys are the output column names
   * in the task result — for semantic_query tasks, that's `<view>__<field>`.
   */
  formats?: Record<string, DisplayFormat>;
};

export type MarkdownDisplay = {
  type: "markdown";
  content: string;
};

type ErrorDisplay = {
  type: "error";
  title: string;
  error: string;
};

type RowDisplay = {
  type: "row";
  /** Number of equal-width columns; defaults to the number of children */
  columns?: number;
  children: Display[];
};

export type Display =
  | ErrorDisplay
  | BarChartDisplay
  | LineChartDisplay
  | TableDisplay
  | PieChartDisplay
  | MarkdownDisplay
  | RowDisplay;

export type TableData = {
  file_path: string;
  json?: string;
};

type Data = string | number | boolean | null | TableData;

export type DataContainer = Data | DataList | DataMap;

// eslint-disable-next-line @typescript-eslint/no-empty-object-type
interface DataList extends Array<DataContainer> {}
// eslint-disable-next-line @typescript-eslint/no-empty-object-type
interface DataMap extends Record<string, DataContainer> {}
