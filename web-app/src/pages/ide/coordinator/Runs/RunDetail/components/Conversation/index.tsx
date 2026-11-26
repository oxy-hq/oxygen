import {
  Brain,
  ChevronDown,
  ChevronRight,
  ChevronsRight,
  Database,
  GitBranch,
  MessageSquare,
  User,
  Wrench
} from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
import { cn } from "@/libs/shadcn/utils";
import type { RunEventEntry } from "@/services/api/coordinator";
import { formatTokens } from "../../../../components/utils";
import {
  buildWaterfall,
  type ChildSpan,
  colorsFor,
  formatMs,
  type PhaseSpan
} from "../Waterfall/model";

/**
 * Conversation tab — re-renders an agent run as a chat thread for
 * readers who care about *what the agent did*, not how long each
 * span took. Question → per-phase narrative cards → final answer.
 * Tool calls live inline as collapsible blocks under the phase
 * that invoked them.
 */
export const Conversation: React.FC<{
  events: RunEventEntry[];
  question: string;
  answer?: string;
  errorMessage?: string;
}> = ({ events, question, answer, errorMessage }) => {
  const model = useMemo(() => buildWaterfall(events), [events]);

  return (
    <div className='mx-auto max-w-3xl space-y-4 p-4'>
      <UserMessage question={question} />
      {model.phases.length === 0 ? (
        <EmptyAgentTurn />
      ) : (
        model.phases.map((phase) => (
          <PhaseCard key={`${phase.state}-${phase.index}`} phase={phase} />
        ))
      )}
      {errorMessage ? (
        <ErrorTurn message={errorMessage} />
      ) : answer ? (
        <AssistantAnswer answer={answer} />
      ) : null}
    </div>
  );
};

const UserMessage: React.FC<{ question: string }> = ({ question }) => (
  <div className='flex gap-3'>
    <Avatar className='bg-muted text-muted-foreground'>
      <User className='h-4 w-4' />
    </Avatar>
    <div className='flex-1 bg-muted/40 p-3 text-sm'>
      {question || <span className='text-muted-foreground italic'>(no question)</span>}
    </div>
  </div>
);

const EmptyAgentTurn: React.FC = () => (
  <div className='flex gap-3'>
    <Avatar className='bg-primary/10 text-primary'>
      <Brain className='h-4 w-4' />
    </Avatar>
    <div className='flex-1 border border-border border-dashed p-3 text-muted-foreground text-sm'>
      No phase transitions captured for this run — nothing to narrate yet.
    </div>
  </div>
);

const PhaseCard: React.FC<{ phase: PhaseSpan }> = ({ phase }) => {
  const colors = colorsFor(phase.state);

  return (
    <div className='flex gap-3'>
      <Avatar className={cn("bg-primary/10", colors.text)}>
        <Brain className='h-4 w-4' />
      </Avatar>
      <div className='flex-1 border border-border bg-card'>
        <div className='flex items-center gap-2 border-border border-b px-3 py-2'>
          <span className={cn("font-medium text-sm capitalize", colors.text)}>{phase.state}</span>
          <span className='text-muted-foreground text-xs tabular-nums'>
            {formatMs(phase.durationMs)} · {phase.llmCalls} LLM call
            {phase.llmCalls === 1 ? "" : "s"} · {formatTokens(phase.totalTokens)} tok
          </span>
        </div>
        <div className='space-y-1.5 px-3 py-2'>
          {phase.children.length === 0 ? (
            <p className='text-muted-foreground text-xs italic'>(no recorded activity)</p>
          ) : (
            phase.children.map((child) => <ChildBlock key={child.id} child={child} />)
          )}
        </div>
      </div>
    </div>
  );
};

const ChildBlock: React.FC<{ child: ChildSpan }> = ({ child }) => {
  const [expanded, setExpanded] = useState(false);

  if (child.kind === "llm") {
    return (
      <div className='flex items-center gap-2 text-muted-foreground text-xs'>
        <Brain className='h-3.5 w-3.5 shrink-0' />
        <span>LLM round</span>
        <span className='tabular-nums'>· {formatMs(child.durationMs)}</span>
        {child.llm && (
          <span className='tabular-nums'>
            · {formatTokens(child.llm.outputTokens)} out
            {child.llm.cacheReadTokens > 0
              ? ` · ${formatTokens(child.llm.cacheReadTokens)} cache`
              : ""}
          </span>
        )}
      </div>
    );
  }

  if (child.kind === "thinking") {
    return (
      <div className='flex items-center gap-2 text-muted-foreground text-xs italic'>
        <Brain className='h-3.5 w-3.5 shrink-0' />
        <span>thinking{child.thinking?.state ? ` (${child.thinking.state})` : ""}</span>
        {child.durationMs > 0 && (
          <span className='not-italic tabular-nums'>· {formatMs(child.durationMs)}</span>
        )}
      </div>
    );
  }

  if (child.kind === "query" && child.query) {
    const q = child.query;
    return (
      <div className='border border-dot-success/40 bg-dot-success/5'>
        <button
          type='button'
          onClick={() => setExpanded((v) => !v)}
          className='flex w-full items-center gap-2 px-2 py-1.5 text-left text-xs hover:bg-dot-success/10'
        >
          {expanded ? (
            <ChevronDown className='h-3 w-3 shrink-0' />
          ) : (
            <ChevronRight className='h-3 w-3 shrink-0' />
          )}
          <Database className='h-3.5 w-3.5 shrink-0 text-status-success' />
          <span className='truncate font-medium'>{child.label}</span>
          <span className='ml-auto text-muted-foreground tabular-nums'>
            {formatMs(child.durationMs)}
            {q.success ? ` · ${q.rowCount.toLocaleString()} rows` : ""}
          </span>
          {q.isPreagg && (
            <span className='bg-dot-success/15 px-1.5 py-0.5 text-status-success text-xs'>
              preagg
            </span>
          )}
          {!q.success && (
            <span className='bg-destructive/15 px-1.5 py-0.5 text-destructive text-xs'>failed</span>
          )}
        </button>
        {expanded && (
          <div className='space-y-1.5 border-dot-success/30 border-t px-2 py-1.5'>
            {q.sql && <ToolPreview label={`sql · ${q.source}`} value={q.sql} />}
            {q.error && <ToolPreview label='error' value={q.error} variant='error' />}
          </div>
        )}
      </div>
    );
  }

  if (child.kind === "step") {
    return (
      <div className='flex items-center gap-2 text-muted-foreground text-xs'>
        <ChevronsRight className='h-3.5 w-3.5 shrink-0 text-status-info' />
        <span className='truncate font-medium text-foreground'>{child.label}</span>
        <span className='tabular-nums'>· {formatMs(child.durationMs)}</span>
        {child.step?.success === false && (
          <span className='bg-destructive/15 px-1.5 py-0.5 text-destructive text-xs'>failed</span>
        )}
      </div>
    );
  }

  if (child.kind === "subrun" && child.subrun) {
    const sub = child.subrun;
    return (
      <div className='border border-chart-3/40 bg-chart-3/5'>
        <button
          type='button'
          onClick={() => setExpanded((v) => !v)}
          className='flex w-full items-center gap-2 px-2 py-1.5 text-left text-xs hover:bg-chart-3/10'
        >
          {expanded ? (
            <ChevronDown className='h-3 w-3 shrink-0' />
          ) : (
            <ChevronRight className='h-3 w-3 shrink-0' />
          )}
          <GitBranch className='h-3.5 w-3.5 shrink-0 text-chart-3' />
          <span className='font-medium'>delegated → {sub.target}</span>
          <span className='ml-auto text-muted-foreground tabular-nums'>
            {formatMs(child.durationMs)} · {sub.nested.phases.length} phase
            {sub.nested.phases.length === 1 ? "" : "s"}
          </span>
          <span
            className={cn(
              "px-1.5 py-0.5 text-xs",
              sub.success
                ? "bg-dot-success/15 text-status-success"
                : "bg-destructive/15 text-destructive"
            )}
          >
            {sub.success ? "ok" : "failed"}
          </span>
        </button>
        {expanded && (
          <div className='space-y-1.5 border-chart-3/30 border-t px-2 py-1.5'>
            {sub.request && <ToolPreview label='request' value={sub.request} />}
            {sub.answer && <ToolPreview label='answer' value={sub.answer} />}
            {sub.error && <ToolPreview label='error' value={sub.error} variant='error' />}
            {sub.nested.phases.length > 0 && (
              <div className='space-y-0.5 border border-border bg-card p-1.5'>
                {sub.nested.phases.map((np) => (
                  <div
                    key={`${np.state}-${np.index}`}
                    className='flex items-center justify-between text-muted-foreground text-xs'
                  >
                    <span className='capitalize'>{np.state}</span>
                    <span className='tabular-nums'>
                      {formatMs(np.durationMs)} · {np.llmCalls} LLM · {np.toolCalls} tool
                    </span>
                  </div>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
    );
  }

  return (
    <div className='border border-border bg-muted/20'>
      <button
        type='button'
        onClick={() => setExpanded((v) => !v)}
        className='flex w-full items-center gap-2 px-2 py-1.5 text-left text-xs hover:bg-muted/40'
      >
        {expanded ? (
          <ChevronDown className='h-3 w-3 shrink-0' />
        ) : (
          <ChevronRight className='h-3 w-3 shrink-0' />
        )}
        <Wrench className='h-3.5 w-3.5 shrink-0 text-status-warning' />
        <span className='font-medium'>{child.label}</span>
        <span className='ml-auto text-muted-foreground tabular-nums'>
          {formatMs(child.durationMs)}
        </span>
        {child.status === "error" && (
          <span className='bg-destructive/15 px-1.5 py-0.5 text-destructive'>error</span>
        )}
      </button>
      {expanded && child.tool && (
        <div className='space-y-1.5 border-border border-t px-2 py-1.5'>
          <ToolPreview label='input' value={child.tool.input} />
          {child.tool.error ? (
            <ToolPreview label='error' value={child.tool.error} variant='error' />
          ) : (
            <ToolPreview label='output' value={child.tool.output} />
          )}
        </div>
      )}
    </div>
  );
};

const ToolPreview: React.FC<{
  label: string;
  value: unknown;
  variant?: "error";
}> = ({ label, value, variant }) => {
  const text =
    typeof value === "string"
      ? value
      : value === null || value === undefined
        ? "—"
        : JSON.stringify(value, null, 2);
  return (
    <div>
      <p className='mb-0.5 text-muted-foreground text-xs uppercase tracking-wide'>{label}</p>
      <pre
        className={cn(
          "max-h-40 overflow-y-auto whitespace-pre-wrap break-words bg-card p-1.5 font-mono text-xs",
          variant === "error" && "text-destructive"
        )}
      >
        {text}
      </pre>
    </div>
  );
};

const AssistantAnswer: React.FC<{ answer: string }> = ({ answer }) => (
  <div className='flex gap-3'>
    <Avatar className='bg-primary/10 text-primary'>
      <MessageSquare className='h-4 w-4' />
    </Avatar>
    <div className='flex-1 whitespace-pre-wrap border border-primary/30 bg-primary/5 p-3 text-sm'>
      {answer}
    </div>
  </div>
);

const ErrorTurn: React.FC<{ message: string }> = ({ message }) => (
  <div className='flex gap-3'>
    <Avatar className='bg-destructive/15 text-destructive'>
      <MessageSquare className='h-4 w-4' />
    </Avatar>
    <div className='flex-1 whitespace-pre-wrap border border-destructive/40 bg-destructive/5 p-3 text-destructive text-sm'>
      {message}
    </div>
  </div>
);

const Avatar: React.FC<{ children: React.ReactNode; className?: string }> = ({
  children,
  className
}) => (
  <div className={cn("flex h-7 w-7 shrink-0 items-center justify-center rounded-full", className)}>
    {children}
  </div>
);
