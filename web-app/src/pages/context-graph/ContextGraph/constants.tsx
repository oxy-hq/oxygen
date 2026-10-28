import { Workflow as AutomationIcon, BookOpen, Bot, Box, Eye, FileCode, Table } from "lucide-react";

export const NODE_TYPE_CLASSES: Record<string, string> = {
  agent: "bg-graph-agent-bg border-graph-agent-border text-graph-agent-border",
  procedure: "bg-graph-procedure-bg border-graph-procedure-border text-graph-procedure-border",
  workflow: "bg-graph-procedure-bg border-graph-procedure-border text-graph-procedure-border",
  automation: "bg-graph-automation-bg border-graph-automation-border text-graph-automation-border",
  topic: "bg-graph-automation-bg border-graph-automation-border text-graph-automation-border",
  view: "bg-graph-view-bg border-graph-view-border text-graph-view-border",
  sql_query: "bg-graph-sql-query-bg border-graph-sql-query-border text-graph-sql-query-border",
  table: "bg-graph-table-bg border-graph-table-border text-graph-table-border",
  entity: "bg-graph-entity-bg border-graph-entity-border text-graph-entity-border"
};

export const HANDLE_STYLE_HIDDEN = {
  width: 0,
  height: 0,
  minWidth: 0,
  minHeight: 0,
  opacity: 0,
  border: "none",
  background: "transparent",
  padding: 0
} as const;

export const HANDLE_STYLE_VISIBLE = {
  width: 8,
  height: 8,
  border: "2px solid var(--muted-foreground)",
  background: "var(--background)",
  opacity: 0.6
} as const;

export const ICONS: Record<string, React.ReactNode> = {
  agent: <Bot className='h-3.5 w-3.5' />,
  procedure: <AutomationIcon className='h-3.5 w-3.5' />,
  workflow: <AutomationIcon className='h-3.5 w-3.5' />,
  automation: <AutomationIcon className='h-3.5 w-3.5' />,
  topic: <BookOpen className='h-3.5 w-3.5' />,
  view: <Eye className='h-3.5 w-3.5' />,
  sql_query: <FileCode className='h-3.5 w-3.5' />,
  table: <Table className='h-3.5 w-3.5' />,
  entity: <Box className='h-3.5 w-3.5' />
};

export const TYPE_ORDER = [
  "entity",
  "agent",
  "procedure",
  "workflow",
  "automation",
  "topic",
  "view",
  "sql_query",
  "table"
];

export const TYPE_LABEL_SINGULAR: Record<string, string> = {
  agent: "Agent",
  workflow: "Automation",
  procedure: "Automation",
  topic: "Topic",
  view: "View",
  sql_query: "SQL Query",
  table: "Table",
  entity: "Entity",
  automation: "Automation"
};

export const TYPE_LABELS: Record<string, string> = {
  agent: "Agents",
  procedure: "Automations (.procedure.yml)",
  workflow: "Automations (.automation.yml)",
  automation: "Automations",
  topic: "Topics",
  view: "Views",
  sql_query: "SQL Queries",
  table: "Tables",
  entity: "Entities"
};

export type FocusType =
  | "auto"
  | "agent"
  | "procedure"
  | "workflow"
  | "automation"
  | "topic"
  | "view"
  | "sql_query"
  | "table"
  | "entity";

export const FOCUS_OPTIONS: Array<{ value: FocusType; label: string; icon?: React.ReactNode }> = [
  { value: "auto", label: "All Types" },
  { value: "agent", label: "Agents", icon: <Bot className='h-4 w-4' /> },
  {
    value: "automation",
    label: "Automations",
    icon: <AutomationIcon className='h-4 w-4' />
  },
  {
    value: "procedure",
    label: "Automations (.procedure.yml)",
    icon: <AutomationIcon className='h-4 w-4' />
  },
  {
    value: "workflow",
    label: "Automations (.automation.yml)",
    icon: <AutomationIcon className='h-4 w-4' />
  },
  { value: "topic", label: "Topics", icon: <BookOpen className='h-4 w-4' /> },
  { value: "view", label: "Views", icon: <Eye className='h-4 w-4' /> },
  { value: "sql_query", label: "SQL Queries", icon: <FileCode className='h-4 w-4' /> },
  { value: "table", label: "Tables", icon: <Table className='h-4 w-4' /> },
  { value: "entity", label: "Entities", icon: <Box className='h-4 w-4' /> }
];

export const ROW_HEIGHT = 80;
export const MIN_NODE_WIDTH = 150;
export const PADDING = 40;
export const MAX_ROW_WIDTH = 1400;
