import type { Trace } from "@/services/api/traces";

export interface TraceChartsProps {
  traces: Trace[] | undefined;
  /** How many traces match the filters. `traces` holds only the newest of them. */
  total: number | undefined;
  isLoading: boolean;
}

export interface TimeBucket {
  time: string;
  automationCount: number;
  analyticsCount: number;
  tokens: number;
}

export interface DurationBucket {
  range: string;
  count: number;
}

export interface TraceStats {
  automationRuns: number;
  analyticsRuns: number;
  avgDuration: string;
  totalTokens: number;
}
