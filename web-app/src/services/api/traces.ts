import { apiClient } from "./axios";

export interface Trace {
  traceId: string;
  spanId: string;
  timestamp: string;
  spanName: string;
  serviceName: string;
  durationNs: number;
  statusCode: string;
  statusMessage: string;
  spanKind: string;
  spanAttributes: Array<[string, string]>;
  eventsAttributes: Array<Array<[string, string]>>;
  promptTokens?: number;
  completionTokens?: number;
  totalTokens?: number;
}

function tupleArrayToRecord(arr: Array<[string, string]>): Record<string, string> {
  return Object.fromEntries(arr);
}

// Helper functions to parse trace data
export function getAgentRef(trace: Trace): string | undefined {
  const attrs = tupleArrayToRecord(trace.spanAttributes);
  return attrs["oxy.agent.ref"];
}

export function getPrompt(trace: Trace): string | undefined {
  for (const eventAttrs of trace.eventsAttributes) {
    const attrs = tupleArrayToRecord(eventAttrs);
    if (attrs.name === "run_agent.input") {
      return attrs.prompt;
    }
  }
  return undefined;
}

export function getTokensTotal(trace: Trace): number | undefined {
  return trace.totalTokens;
}

export function getDurationMs(trace: Trace): number {
  return trace.durationNs / 1_000_000;
}

export function getSpanAttributesAsRecord(trace: Trace): Record<string, string> {
  return tupleArrayToRecord(trace.spanAttributes);
}

interface Span {
  spanId: string;
  parentSpanId: string;
  spanName: string;
  timestamp: string;
  durationMs: number;
  statusCode: string;
  attributes: string;
}

export interface TraceDetail {
  trace_id: string;
  spans: Span[];
  total_duration_ms: number;
  start_time: string;
  end_time: string;
}

export interface ClusterMapPoint {
  traceId: string;
  question: string;
  x: number;
  y: number;
  clusterId: number;
  intentName: string;
  confidence: number;
  timestamp: string;
  durationMs?: number;
  status?: "ok" | "error" | "unset";
}

export interface ClusterSummary {
  clusterId: number;
  intentName: string;
  description: string;
  count: number;
  color: string;
  sampleQuestions: string[];
}

export interface ClusterMapResponse {
  points: ClusterMapPoint[];
  clusters: ClusterSummary[];
  totalPoints: number;
  outlierCount: number;
}

// Trace detail span from API (raw ClickHouse data)
// Note: spanAttributes and eventsAttributes come as arrays of [key, value] tuples from ClickHouse Map type
export interface TraceDetailSpan {
  timestamp: string;
  traceId: string;
  spanId: string;
  parentSpanId: string;
  spanName: string;
  spanKind: string;
  serviceName: string;
  spanAttributes: Array<[string, string]>;
  duration: number; // nanoseconds
  statusCode: string;
  statusMessage: string;
  eventsName: string[];
  eventsAttributes: Array<Array<[string, string]>>;
}

// Helper to convert [key, value] tuples to Record
export function tuplesToRecord(tuples: Array<[string, string]>): Record<string, string> {
  return Object.fromEntries(tuples);
}

export interface SpanEvent {
  timestamp: string;
  name: string;
  attributes: Record<string, string>;
}

export interface TimelineSpan {
  spanId: string;
  parentSpanId: string;
  spanName: string;
  timestamp: string;
  durationMs: number;
  offsetMs: number;
  depth: number;
  statusCode: string;
  spanKind: string;
  attributes: Record<string, string>;
  events: SpanEvent[];
  children: string[];
}

export interface PaginatedTraceResponse {
  items: Trace[];
  total: number;
  limit: number;
  offset: number;
}

export class TracesService {
  static async listTraces(
    projectId: string,
    limit?: number,
    offset?: number,
    status?: string,
    duration?: string,
    search?: string,
    from?: number,
    to?: number
  ): Promise<PaginatedTraceResponse> {
    const params = new URLSearchParams();
    if (limit !== undefined) params.append("limit", limit.toString());
    if (offset !== undefined) params.append("offset", offset.toString());
    if (status && status !== "all") params.append("status", status);
    // Absolute range (Theme 3) overrides the preset duration when both endpoints set.
    if (from !== undefined && to !== undefined) {
      params.append("from", from.toString());
      params.append("to", to.toString());
    } else if (duration && duration !== "all") {
      params.append("duration", duration);
    }
    const trimmed = search?.trim();
    if (trimmed) params.append("search", trimmed);

    let url = `/${projectId}/traces`;
    const paramsStr = params.toString();
    if (paramsStr) {
      url += `?${paramsStr}`;
    }
    const response = await apiClient.get(url);
    return response.data;
  }

  static async getTrace(projectId: string, traceId: string): Promise<TraceDetail> {
    const response = await apiClient.get(`/${projectId}/traces/${traceId}`);
    return response.data;
  }

  static async getTraceDetail(projectId: string, traceId: string): Promise<TraceDetailSpan[]> {
    const response = await apiClient.get(`/${projectId}/traces/${traceId}`);
    return response.data;
  }

  static async getClusterMap(
    projectId: string,
    limit?: number,
    days?: number,
    source?: string
  ): Promise<ClusterMapResponse> {
    const params = new URLSearchParams();
    if (limit !== undefined) params.append("limit", limit.toString());
    if (days !== undefined) params.append("days", days.toString());
    if (source) params.append("source", source);

    let url = `/${projectId}/traces/clusters/map`;
    const paramsStr = params.toString();
    if (paramsStr) {
      url += `?${paramsStr}`;
    }
    const response = await apiClient.get(url);
    return response.data;
  }
}
