import type { SemanticQueryFilter } from "@/services/api/semantic";

interface Dimension {
  name: string;
  type: string;
  description?: string;
  expr: string;
}

interface Measure {
  name: string;
  type: string;
  description?: string;
  expr?: string;
  induced?: boolean;
  promoted_from?: string;
}

export interface ViewData {
  name: string;
  description?: string;
  datasource: string;
  table: string;
  dimensions: Dimension[];
  measures: Measure[];
}

export interface TopicData {
  name: string;
  description?: string;
  views: string[];
  base_view?: string;
}

export interface ViewWithData extends ViewData {
  viewName: string;
}

export type Field = {
  name: string;
  fullName: string;
  type?: "string" | "number" | "date" | "datetime" | "boolean";
};

export type Order = {
  field: string;
  direction: "asc" | "desc";
};

export type Filter = SemanticQueryFilter;
