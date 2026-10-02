import { AlertTriangle, Bot, Database, GitBranch, Repeat, Wrench } from "lucide-react";
import type React from "react";
import { cn } from "@/libs/shadcn/utils";
import { toText } from "@/libs/utils/string";
import { formatDurationMs } from "../../../../components/utils";
import { Waterfall } from "../Waterfall";
import type { AutomationNode } from "./model";

const KIND_ICON: Record<string, React.ElementType> = {
  sql: Database,
  agent: Bot,
  automation: GitBranch,
  loop: Repeat,
  generic: Wrench
};

/**
 * Side panel for the currently selected automation step. Routes by
 * `node.kind`: SQL gets a query view with first-rows preview, agent /
 * automation get the existing `Waterfall` embedded against the step's
 * scoped event log, generic falls back to status + error + duration.
 *
 * Reusing `<Waterfall events=…/>` here is the load-bearing move — an
 * automation that delegates to an analytics agent gets the same
 * phase/LLM/tool/cost render the standalone agent run page uses, with
 * zero duplicate code.
 */
export const StepInspector: React.FC<{ node: AutomationNode | null }> = ({ node }) => {
  if (!node) {
    return (
      <div className='flex h-32 items-center justify-center px-4 text-muted-foreground text-xs'>
        Select a step to inspect.
      </div>
    );
  }

  const Icon = KIND_ICON[node.kind] ?? Wrench;
  const durationLabel = node.durationMs !== null ? formatDurationMs(node.durationMs) : "running…";

  return (
    <div className='space-y-3 p-4'>
      <div className='flex items-center gap-2'>
        <Icon className='h-4 w-4 text-primary' />
        <span className='truncate font-semibold text-sm'>{node.name}</span>
        <span className='ml-auto text-muted-foreground text-xs tabular-nums'>{durationLabel}</span>
      </div>

      <div className='flex flex-wrap gap-x-3 gap-y-1 text-muted-foreground text-xs'>
        {node.taskType && (
          <span>
            <span className='font-medium text-foreground'>type</span> {node.taskType}
          </span>
        )}
        <span>
          <span className='font-medium text-foreground'>status</span> {node.status}
        </span>
        {node.cached && (
          <span className='rounded bg-cyan-500/15 px-1 py-0.5 text-cyan-700'>cached</span>
        )}
        {node.children.length > 0 && (
          <span>
            <span className='font-medium text-foreground'>child steps</span> {node.children.length}
          </span>
        )}
        {node.kind === "agent" && node.nestedWaterfall && (
          <span>
            <span className='font-medium text-foreground'>phases</span>{" "}
            {node.nestedWaterfall.phases.length}
          </span>
        )}
      </div>

      {node.error && (
        <div className='flex items-start gap-2 rounded border border-destructive/40 bg-destructive/5 p-2 text-destructive text-xs'>
          <AlertTriangle className='mt-0.5 h-3.5 w-3.5 shrink-0' />
          <span className='break-words'>{node.error}</span>
        </div>
      )}

      {node.kind === "sql" && node.query && <QueryDetail query={node.query} />}

      {(node.kind === "agent" || node.kind === "automation") && node.events.length > 0 && (
        <div className='space-y-1'>
          <p className='text-muted-foreground text-xs uppercase tracking-wide'>nested trace</p>
          <div className='rounded border border-border bg-muted/20'>
            {/* Reuse the agent-run waterfall against this step's scoped
                event slice — same phase / LLM / tool / SQL rendering. */}
            <Waterfall events={node.events} />
          </div>
        </div>
      )}

      {node.kind === "generic" && node.children.length === 0 && (
        <p className='text-muted-foreground text-xs italic'>
          No type-specific detail captured for this step. Status, duration, and any error are shown
          above.
        </p>
      )}

      {node.children.length > 0 && <ContainerRollup node={node} />}
    </div>
  );
};

/** Loop / sub-automation container rollup — counts iterations by status
 *  + sums measured durations so the inspector reads as a useful
 *  summary even before the user drills into a specific iteration. */
const ContainerRollup: React.FC<{ node: AutomationNode }> = ({ node }) => {
  const total = node.children.length;
  const succeeded = node.children.filter((c) => c.status === "succeeded").length;
  const failed = node.children.filter((c) => c.status === "failed").length;
  const running = node.children.filter((c) => c.status === "running").length;
  const pending = node.children.filter((c) => c.status === "pending").length;
  const sumMs = node.children.reduce((acc, c) => acc + (c.durationMs ?? 0), 0);
  const isLoop = node.kind === "loop";
  const label = isLoop ? "iteration" : "child step";

  return (
    <div className='space-y-2'>
      <div className='flex flex-wrap gap-x-3 gap-y-1 text-muted-foreground text-xs'>
        <span>
          <span className='font-medium text-foreground'>{total}</span> {label}
          {total === 1 ? "" : "s"}
        </span>
        {succeeded > 0 && <span className='text-emerald-600'>✓ {succeeded}</span>}
        {failed > 0 && <span className='text-destructive'>✗ {failed}</span>}
        {running > 0 && <span className='text-primary'>running {running}</span>}
        {pending > 0 && <span>pending {pending}</span>}
        {sumMs > 0 && <span className='tabular-nums'>· total {formatDurationMs(sumMs)}</span>}
      </div>
      <p className='text-muted-foreground text-xs italic'>Click any {label} below to drill in.</p>
    </div>
  );
};

const QueryDetail: React.FC<{
  query: NonNullable<AutomationNode["query"]>;
}> = ({ query }) => (
  <div className='space-y-2'>
    <div className='flex flex-wrap gap-x-3 gap-y-1 text-muted-foreground text-xs'>
      <span>
        <span className='font-medium text-foreground'>source</span> {query.source}
      </span>
      {query.success ? (
        <span className='text-emerald-600'>
          ✓ {query.rowCount.toLocaleString()} row{query.rowCount === 1 ? "" : "s"}
          {query.columns.length > 0 ? ` · ${query.columns.length} cols` : ""}
        </span>
      ) : (
        <span className='text-destructive'>✗ failed</span>
      )}
    </div>

    {query.sql && (
      <div>
        <p className='mb-0.5 text-muted-foreground text-xs uppercase tracking-wide'>sql</p>
        <pre className='max-h-64 overflow-y-auto whitespace-pre-wrap break-words rounded bg-muted/40 p-2 font-mono text-xs'>
          {query.sql}
        </pre>
      </div>
    )}

    {query.success && query.rowsPreview.length > 0 && (
      <ResultPreview columns={query.columns} rows={query.rowsPreview} />
    )}
  </div>
);

const ResultPreview: React.FC<{ columns: string[]; rows: unknown[][] }> = ({ columns, rows }) => {
  const display = rows.slice(0, 8);
  return (
    <div>
      <p className='mb-0.5 text-muted-foreground text-xs uppercase tracking-wide'>result</p>
      <div className='overflow-x-auto rounded border border-border bg-card'>
        <table className='w-full text-xs'>
          <thead>
            <tr className='border-border border-b'>
              {columns.map((col) => (
                <th key={col} className='px-2 py-1 text-left font-medium text-muted-foreground'>
                  {col}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {display.map((row) => {
              const rowKey = JSON.stringify(row);
              return (
                <tr key={rowKey} className={cn("border-border border-b last:border-0")}>
                  {row.map((cell, j) => (
                    <td
                      key={columns[j] ?? `c${j}`}
                      className='whitespace-nowrap px-2 py-1 tabular-nums'
                    >
                      {cell === null || cell === undefined ? (
                        <span className='text-muted-foreground italic'>null</span>
                      ) : typeof cell === "object" ? (
                        JSON.stringify(cell)
                      ) : (
                        toText(cell)
                      )}
                    </td>
                  ))}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {rows.length > display.length && (
        <p className='mt-1 text-muted-foreground text-xs italic'>
          showing first {display.length} of {rows.length} preview rows
        </p>
      )}
    </div>
  );
};
