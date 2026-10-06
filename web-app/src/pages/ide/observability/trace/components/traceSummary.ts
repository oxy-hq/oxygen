import type { TimelineSpan } from "@/services/api/traces";
import { getSpanCategory, isEmptyToolSpan, isErrorStatus } from "./spanCategory";

export interface TraceSummary {
  spanCount: number;
  errorCount: number;
  llmCallCount: number;
  toolCallCount: number;
  totalTokens: number;
}

/**
 * The event the LLM client emits once per completed call. The trace list sums
 * its token total off the same event (`list_traces` in the ClickHouse backend),
 * so a trace's card and its detail page agree on the number.
 */
const LLM_USAGE_EVENT = "llm.usage";

function usageTokens(span: TimelineSpan): number {
  let tokens = 0;
  for (const event of span.events) {
    if (event.name !== LLM_USAGE_EVENT) continue;
    const total = Number.parseInt(event.attributes.total_tokens ?? "", 10);
    if (!Number.isNaN(total)) tokens += total;
  }
  return tokens;
}

/**
 * The summary strip's figures, computed from the spans the waterfall is drawn
 * from.
 *
 * These used to be fetched from `GET …/traces/{id}/waterfall`, a route the
 * server never had: the strip was skipped on every trace and Compare could only
 * say it could not load. The spans are already on the page, and counting them
 * here is what keeps the strip and the waterfall below it from disagreeing:
 * "LLM calls" and "Tool calls" are the rows the waterfall colours as `llm` and
 * `tool`, which is why a tool span the waterfall hides is not counted either.
 */
export function summarizeTrace(spans: TimelineSpan[]): TraceSummary {
  const summary: TraceSummary = {
    spanCount: spans.length,
    errorCount: 0,
    llmCallCount: 0,
    toolCallCount: 0,
    totalTokens: 0
  };
  for (const span of spans) {
    if (isErrorStatus(span.statusCode)) summary.errorCount += 1;
    const category = getSpanCategory(span);
    if (category === "llm") summary.llmCallCount += 1;
    else if (category === "tool" && !isEmptyToolSpan(span)) summary.toolCallCount += 1;
    summary.totalTokens += usageTokens(span);
  }
  return summary;
}
