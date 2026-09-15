import type { LogItem } from "@/services/types";
import type { Artifact } from "@/types/artifact";

export interface TextContent {
  type: "text";
  content: string;
}

export interface DataAppContent {
  type: "data_app";
  content: string;
}

interface AutomationArtifactKind {
  type: "workflow";
  value: {
    ref: string;
  };
}

interface AgentArtifactKind {
  type: "agent";
  value: {
    ref: string;
  };
}

interface ExecuteSQLArtifactKind {
  type: "execute_sql";
  value: {
    database: string;
  };
}

interface SemanticQueryArtifactKind {
  type: "semantic_query";
  value: {
    database: string;
  };
}

interface OmniQueryArtifactKind {
  type: "omni_query";
  value: {
    topic: string;
    integration: string;
  };
}

interface LookerQueryArtifactKind {
  type: "looker_query";
  value: {
    model: string;
    explore: string;
    integration: string;
  };
}

interface SandboxAppArtifactKind {
  type: "sandbox_app";
  value: {
    type: "v0";
    metadata: {
      chat_id: string;
    };
  };
}

type ArtifactKind =
  | AutomationArtifactKind
  | AgentArtifactKind
  | ExecuteSQLArtifactKind
  | SemanticQueryArtifactKind
  | OmniQueryArtifactKind
  | LookerQueryArtifactKind
  | SandboxAppArtifactKind;

export interface ArtifactStartedContent {
  type: "artifact_started";
  id: string;
  title: string;
  kind: ArtifactKind;
}

interface AutomationArtifactValue {
  type: "log_item";
  value: LogItem;
}

interface AgentArtifactValue {
  type: "content";
  value: string;
}

interface ExecuteSQLArtifactValue {
  type: "execute_sql";
  value: {
    database: string;
    sql_query: string;
    result?: string[][];
    result_file?: string;
    is_result_truncated: boolean;
  };
}

interface SemanticQueryArtifactValue {
  type: "semantic_query";
  value: {
    database: string;
    sql_query: string;
    result?: string[][];
    result_file?: string;
    is_result_truncated: boolean;
    topic: string;
    dimensions: string[];
    measures: string[];
    filters: Array<{
      field: string;
      op: string;
      value: string | number | boolean | string[] | number[];
    }>;
    orders: Array<{
      field: string;
      direction: string;
    }>;
    limit?: number;
    offset?: number;
  };
}

interface LookerQueryArtifactValue {
  type: "looker_query";
  value: {
    model: string;
    explore: string;
    sql: string;
    result?: string[][];
    result_file?: string;
    is_result_truncated: boolean;
    fields: string[];
    filters?: Record<string, string>;
    sorts?: string[];
    limit?: number;
  };
}

interface SandboxAppArtifactValue {
  type: "sandbox_info";
  value: {
    preview_url: string;
  };
}

type ArtifactValue =
  | AutomationArtifactValue
  | AgentArtifactValue
  | ExecuteSQLArtifactValue
  | SemanticQueryArtifactValue
  | LookerQueryArtifactValue
  | SandboxAppArtifactValue;

export interface ArtifactValueContent {
  type: "artifact_value";
  id: string;
  value: ArtifactValue;
}

export interface ArtifactDoneContent {
  type: "artifact_done";
  id: string;
}

interface ErrorContent {
  type: "error";
  message: string;
}

export interface UsageContent {
  type: "usage";
  usage: Usage;
}

/**
 * Reasoning span lifecycle. The agent emits ReasoningStarted on the first
 * delta, a ReasoningChunk for every token, and ReasoningDone when the span
 * closes. The MessageProcessor synthesizes the legacy `:::reasoning` /
 * `:::` markdown directive from these so the existing ReasoningPlugin
 * keeps rendering without changes.
 */
export interface ReasoningStartedContent {
  type: "reasoning_started";
  id: string;
}

export interface ReasoningChunkContent {
  type: "reasoning_chunk";
  id: string;
  delta: string;
}

interface ReasoningDoneContent {
  type: "reasoning_done";
  id: string;
}

/**
 * A chart artifact emitted by the visualize tool. Replaces the legacy
 * `:chart{chart_src=…}` text directive on the live stream. The
 * MessageProcessor synthesizes the directive into the message body so
 * the existing ChartPlugin renders it without changes.
 */
export interface ChartContent {
  type: "chart";
  chart_src: string;
}

type AnswerContent =
  | TextContent
  | ArtifactStartedContent
  | ArtifactValueContent
  | ArtifactDoneContent
  | UsageContent
  | ErrorContent
  | DataAppContent
  | ReasoningStartedContent
  | ReasoningChunkContent
  | ReasoningDoneContent
  | ChartContent;

export type Answer = {
  content: AnswerContent;
  references: Reference[];
  step: string;
  is_error: boolean;
};

export type Reference = (SqlQueryReference | DataAppReference) & {
  type: ReferenceType;
};

export enum ReferenceType {
  SQLQuery = "sqlQuery",
  DataApp = "dataApp"
}

export type SqlQueryReference = {
  type: ReferenceType.SQLQuery;
  database: string;
  sql_query: string;
  result?: string[][];
  result_file?: string;
  is_result_truncated: boolean;
};

type DataAppReference = {
  type: ReferenceType.DataApp;
  file_path: string;
};

export type ThreadItem = {
  id: string;
  title: string;
  input: string;
  output: string;
  source: string;
  source_type: string;
  created_at: string;
  references: Reference[];
  is_processing: boolean;
  sandbox_info?: Record<string, unknown>;
};

export type ThreadCreateRequest = {
  title: string;
  input: string;
  source: string;
  source_type: string;
};

export type PaginationInfo = {
  page: number;
  limit: number;
  total: number;
  total_pages: number;
  has_next: boolean;
  has_previous: boolean;
};

export type ThreadsResponse = {
  threads: ThreadItem[];
  pagination: PaginationInfo;
};

type Usage = {
  inputTokens: number;
  outputTokens: number;
};

export interface Message {
  id: string;
  content: string;
  references: Reference[];
  steps: string[];
  is_human: boolean;
  isStreaming: boolean;
  thread_id: string;
  usage: Usage;
  artifacts: { [key: string]: Artifact };
  created_at: string;
  file_path: string;
}
