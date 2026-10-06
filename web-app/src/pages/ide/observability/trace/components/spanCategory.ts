import type { TimelineSpan } from "@/services/api/traces";

/**
 * Coarse execution category used to color spans in the waterfall / flamegraph.
 * The five named categories map to the validated span palette
 * (`--span-agent` … `--span-retrieval`); `other` falls back to a neutral tone.
 */
export type SpanCategory = "agent" | "llm" | "tool" | "sql" | "retrieval" | "other";

interface SpanCategoryMeta {
  label: string;
  /** Tailwind bg class for the bar / flamegraph cell. */
  barClass: string;
  /** Tailwind bg class for the small legend / row dot. */
  dotClass: string;
  /** CSS var name (for ECharts / canvas contexts that need a resolved color). */
  cssVar: string;
}

export const SPAN_CATEGORY_META: Record<SpanCategory, SpanCategoryMeta> = {
  agent: {
    label: "agent",
    barClass: "bg-span-agent",
    dotClass: "bg-span-agent",
    cssVar: "--span-agent"
  },
  llm: { label: "llm", barClass: "bg-span-llm", dotClass: "bg-span-llm", cssVar: "--span-llm" },
  tool: {
    label: "tool",
    barClass: "bg-span-tool",
    dotClass: "bg-span-tool",
    cssVar: "--span-tool"
  },
  sql: { label: "sql", barClass: "bg-span-sql", dotClass: "bg-span-sql", cssVar: "--span-sql" },
  retrieval: {
    label: "retrieval",
    barClass: "bg-span-retrieval",
    dotClass: "bg-span-retrieval",
    cssVar: "--span-retrieval"
  },
  other: {
    label: "other",
    barClass: "bg-muted-foreground",
    dotClass: "bg-muted-foreground",
    cssVar: "--muted-foreground"
  }
};

/** Categories shown in the waterfall legend (excludes the `other` fallback). */
export const LEGEND_CATEGORIES: SpanCategory[] = ["agent", "llm", "tool", "sql", "retrieval"];

/** Case-insensitive error-status test (raw traces use both "ERROR" and "Error"). */
export function isErrorStatus(statusCode: string | undefined): boolean {
  return !!statusCode && /error/i.test(statusCode);
}

/**
 * A `tool.execute` span that recorded nothing. The waterfall leaves these out,
 * so anything that counts tool calls has to leave them out as well, or the count
 * names rows the reader cannot find.
 */
export function isEmptyToolSpan(span: TimelineSpan): boolean {
  return span.spanName === "tool.execute" && span.events.length === 0;
}

function categoryFromSpanType(spanType: string): SpanCategory | undefined {
  if (spanType === "agent" || spanType === "analytics") return "agent";
  if (spanType === "llm") return "llm";
  if (spanType === "tool_call" || spanType === "tool") return "tool";
  if (spanType === "sql" || spanType === "compile") return "sql";
  if (spanType === "retrieval") return "retrieval";
  return undefined;
}

function categoryFromSpanName(name: string): SpanCategory {
  if (name.includes("llm")) return "llm";
  if (name.includes("retrieval") || name.includes("retrieve") || name.includes("context"))
    return "retrieval";
  if (
    name.includes("sql") ||
    name.includes("compile") ||
    name.includes("semantic_query") ||
    name.includes("execute_sql")
  ) {
    return "sql";
  }
  if (name.includes("tool")) return "tool";
  if (name.startsWith("analytics") || name.startsWith("agent")) return "agent";
  return "other";
}

/** Resolve a span's category, preferring the explicit `oxy.span_type` attribute. */
export function getSpanCategory(span: TimelineSpan): SpanCategory {
  const spanType = span.attributes["oxy.span_type"];
  if (spanType) {
    const mapped = categoryFromSpanType(spanType);
    if (mapped) return mapped;
  }
  return categoryFromSpanName(span.spanName);
}
