import { useState } from "react";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/shadcn/tabs";
import type { ArtifactItem } from "@/hooks/analyticsSteps";
import type { AnalyticsDisplayBlock } from "@/hooks/useAnalyticsRun";
import { toText } from "@/libs/utils/string";
import { ExplainBody } from "@/pages/ide/SemanticLayer/AnomaliesInbox/ExplainDrawer";
import type {
  ExplainResult,
  OpportunityResult,
  PredictResult,
  SensitivityResult
} from "@/types/metricTree";
import { AnalyticsDisplayBlockItem, parseToolJson } from "./analyticsArtifactHelpers";

// ── TimingBar ─────────────────────────────────────────────────────────────────

function formatMs(ms: number): string {
  return ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${ms}ms`;
}

export const TimingBar = ({ item }: { item: ArtifactItem }) => {
  const execMs = item.durationMs;
  const llmMs = item.llmDurationMs;
  if (execMs === undefined && llmMs === undefined) return null;
  return (
    <div className='flex items-center gap-3 border-t px-4 py-2 font-mono text-[10px] text-muted-foreground/70'>
      {llmMs !== undefined && (
        <span>
          <span className='text-muted-foreground/50'>llm </span>
          {formatMs(llmMs)}
        </span>
      )}
      {execMs !== undefined && (
        <span>
          <span className='text-muted-foreground/50'>exec </span>
          {formatMs(execMs)}
        </span>
      )}
      {llmMs !== undefined && execMs !== undefined && (
        <span className='ml-auto'>
          <span className='text-muted-foreground/50'>total </span>
          {formatMs(llmMs + execMs)}
        </span>
      )}
    </div>
  );
};

// ── ChartSection ──────────────────────────────────────────────────────────────

export const ChartSection = ({
  displayBlocks,
  runId = "sidebar"
}: {
  displayBlocks: AnalyticsDisplayBlock[];
  runId?: string;
}) => {
  if (!displayBlocks.length) return null;
  return (
    <div className='shrink-0 space-y-2 border-t p-4'>
      {displayBlocks.map((block, i) => {
        const key = `${block.config.chart_type}-${block.config.title ?? i}`;
        return <AnalyticsDisplayBlockItem key={key} block={block} index={i} runId={runId} />;
      })}
    </div>
  );
};

export const RawArtifactView = ({ item }: { item: ArtifactItem }) => (
  <div className='flex h-full flex-col'>
    <div className='flex-1 overflow-auto p-4'>
      <div className='space-y-3'>
        <div>
          <p className='mb-1 font-medium text-muted-foreground text-xs'>Input</p>
          <pre className='whitespace-pre-wrap rounded border bg-muted/50 p-3 font-mono text-[11px]'>
            {item.toolInput}
          </pre>
        </div>
        {item.toolOutput && (
          <div>
            <p className='mb-1 font-medium text-muted-foreground text-xs'>Output</p>
            <pre className='whitespace-pre-wrap rounded border bg-muted/50 p-3 font-mono text-[11px]'>
              {item.toolOutput}
            </pre>
          </div>
        )}
      </div>
    </div>
    <TimingBar item={item} />
  </div>
);

// ── AskUserView ──────────────────────────────────────────────────────────────

export const AskUserView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ prompt?: string; suggestions?: string[] }>(item.toolInput);
  const prompt = input?.prompt ?? "Question";
  const suggestions = input?.suggestions ?? [];

  const output = parseToolJson<{ answer?: string }>(item.toolOutput);
  const answer = output?.answer ?? null;

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div>
            <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Question</p>
            <p className='text-sm'>{prompt}</p>
          </div>

          {suggestions.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Suggestions</p>
              <div className='flex flex-wrap gap-1.5'>
                {suggestions.map((s) => (
                  <span
                    key={s}
                    className='rounded-full border border-border bg-muted/50 px-2.5 py-0.5 text-xs'
                  >
                    {s}
                  </span>
                ))}
              </div>
            </div>
          )}

          {answer && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>User Response</p>
              <div className='rounded border border-primary/20 bg-primary/5 px-3 py-2'>
                <p className='whitespace-pre-wrap text-sm'>{answer}</p>
              </div>
            </div>
          )}

          {!answer && item.isStreaming && (
            <p className='text-muted-foreground text-xs'>Waiting for user response…</p>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── SearchCatalogView ─────────────────────────────────────────────────────────

type CatalogMetric = { name: string; description?: string };
type CatalogDimension = { name: string; description?: string; type?: string };

export const SearchCatalogView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ queries?: string[] }>(item.toolInput);
  const queries = input?.queries ?? [];

  const output = parseToolJson<{
    metrics?: CatalogMetric[];
    dimensions?: CatalogDimension[];
  }>(item.toolOutput);
  const metrics = output?.metrics ?? [];
  const dimensions = output?.dimensions ?? [];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          {queries.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Search Terms</p>
              <div className='flex flex-wrap gap-1.5'>
                {queries.map((q, i) => (
                  // biome-ignore lint/suspicious/noArrayIndexKey: static list
                  <span key={i} className='rounded-full bg-muted px-2.5 py-0.5 text-xs'>
                    {q}
                  </span>
                ))}
              </div>
            </div>
          )}

          {metrics.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
                Metrics ({metrics.length})
              </p>
              <div className='space-y-1'>
                {metrics.map((m, i) => (
                  // biome-ignore lint/suspicious/noArrayIndexKey: static list
                  <div key={i} className='rounded border bg-muted/30 px-2.5 py-1.5'>
                    <p className='font-medium text-xs'>{m.name}</p>
                    {m.description && (
                      <p className='text-[11px] text-muted-foreground'>{m.description}</p>
                    )}
                  </div>
                ))}
              </div>
            </div>
          )}

          {dimensions.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
                Dimensions ({dimensions.length})
              </p>
              <div className='space-y-1'>
                {dimensions.map((d) => (
                  <div
                    key={d.name}
                    className='flex items-start gap-2 rounded border bg-muted/30 px-2.5 py-1.5'
                  >
                    <div className='min-w-0 flex-1'>
                      <p className='font-medium text-xs'>{d.name}</p>
                      {d.description && (
                        <p className='text-[11px] text-muted-foreground'>{d.description}</p>
                      )}
                    </div>
                    {d.type && (
                      <span className='shrink-0 rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground'>
                        {d.type}
                      </span>
                    )}
                  </div>
                ))}
              </div>
            </div>
          )}

          {metrics.length === 0 && dimensions.length === 0 && item.toolOutput && (
            <p className='text-muted-foreground text-xs'>No results found.</p>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── SearchAutomationsView ──────────────────────────────────────────────────────

type AutomationRefItem = { name: string; path: string; description?: string };

export const SearchAutomationsView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ query?: string }>(item.toolInput);
  const query = input?.query ?? "";

  // Read the canonical key, falling back to the legacy `procedures` key for
  // runs persisted before the procedure→automation rename.
  const output = parseToolJson<{
    automations?: AutomationRefItem[];
    procedures?: AutomationRefItem[];
  }>(item.toolOutput);
  const automations = output?.automations ?? output?.procedures ?? [];

  return (
    <div className='h-full overflow-auto p-4'>
      <div className='space-y-4'>
        {query && (
          <div>
            <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Query</p>
            <span className='rounded-full bg-muted px-2.5 py-0.5 text-xs'>{query}</span>
          </div>
        )}

        {automations.length > 0 ? (
          <div>
            <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
              Automations ({automations.length})
            </p>
            <div className='space-y-1.5'>
              {automations.map((p, i) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: static list
                <div key={i} className='space-y-0.5 rounded border bg-muted/30 px-2.5 py-2'>
                  <p className='font-medium text-xs'>{p.name}</p>
                  <p className='break-all font-mono text-[10px] text-muted-foreground'>{p.path}</p>
                  {p.description && (
                    <p className='text-[11px] text-muted-foreground'>{p.description}</p>
                  )}
                </div>
              ))}
            </div>
          </div>
        ) : (
          item.toolOutput && (
            <p className='text-muted-foreground text-xs'>No matching automations found.</p>
          )
        )}
      </div>
    </div>
  );
};

// ── SampleColumnView ──────────────────────────────────────────────────────────

interface ColumnResult {
  table?: string;
  column?: string;
  data_type?: string;
  sample_values?: unknown[];
  row_count?: number;
  distinct_count?: number;
  min?: string | number;
  max?: string | number;
  avg?: number;
  stdev?: number;
  error?: string;
}

const SingleColumnCard = ({ col }: { col: ColumnResult }) => {
  const table = col.table ?? "";
  const column = col.column ?? "";
  const dataType = col.data_type ?? "";
  const sampleValues = col.sample_values ?? [];

  const fmt = (v: string | number) =>
    typeof v === "number" ? v.toLocaleString(undefined, { maximumFractionDigits: 4 }) : v;

  if (col.error) {
    return (
      <div className='space-y-2'>
        <p className='font-medium font-mono text-xs'>
          {table}.{column}
        </p>
        <p className='text-destructive text-xs'>{col.error}</p>
      </div>
    );
  }

  const meta = [
    { label: "Table", value: table },
    { label: "Column", value: column },
    { label: "Type", value: dataType || "—" },
    col.row_count !== undefined
      ? { label: "Row Count", value: col.row_count.toLocaleString() }
      : null,
    col.distinct_count !== undefined
      ? { label: "Distinct", value: col.distinct_count.toLocaleString() }
      : null,
    col.min !== undefined ? { label: "Min", value: fmt(col.min) } : null,
    col.max !== undefined ? { label: "Max", value: fmt(col.max) } : null,
    col.avg !== undefined ? { label: "Avg", value: fmt(col.avg) } : null,
    col.stdev !== undefined ? { label: "Stdev", value: fmt(col.stdev) } : null
  ].filter(Boolean) as { label: string; value: string | number }[];

  return (
    <div className='space-y-4'>
      <div className='grid grid-cols-2 gap-2'>
        {meta.map((m) => (
          <div key={m.label} className='rounded border bg-muted/30 px-2.5 py-2'>
            <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>{m.label}</p>
            <p className='font-medium font-mono text-xs'>{m.value}</p>
          </div>
        ))}
      </div>

      {sampleValues.length > 0 && (
        <div>
          <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
            Sample Values ({sampleValues.length})
          </p>
          <div className='overflow-hidden rounded border'>
            <table className='w-full text-xs'>
              <thead>
                <tr className='bg-muted/50'>
                  <th className='px-2.5 py-1.5 text-left font-medium text-muted-foreground'>
                    {column || "value"}
                  </th>
                </tr>
              </thead>
              <tbody>
                {sampleValues.map((v, i) => (
                  // biome-ignore lint/suspicious/noArrayIndexKey: stable ordered list
                  <tr key={i} className={i % 2 === 0 ? "bg-background" : "bg-muted/20"}>
                    <td className='px-2.5 py-1 font-mono'>{v == null ? "null" : toText(v)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
};

export const SampleColumnView = ({ item }: { item: ArtifactItem }) => {
  const output = parseToolJson<{ results?: ColumnResult[] }>(item.toolOutput);
  const results = output?.results ?? [];

  if (results.length === 0) {
    return (
      <div className='flex h-full flex-col'>
        <div className='flex-1 p-4'>
          <p className='text-muted-foreground text-xs'>No column results.</p>
        </div>
        <TimingBar item={item} />
      </div>
    );
  }

  const tabLabel = (col: ColumnResult, i: number) => {
    const table = col.table || "?";
    const column = col.column || "?";
    return `${table}.${column}` === "?.?" ? `col-${i}` : `${table}.${column}`;
  };

  return (
    <div className='flex h-full flex-col'>
      <Tabs
        key={item.id}
        defaultValue={tabLabel(results[0], 0)}
        className='flex min-h-0 flex-1 flex-col'
      >
        <TabsList variant='line' className='shrink-0 overflow-x-auto border-b px-4 pt-2'>
          {results.map((col, i) => (
            <TabsTrigger key={tabLabel(col, i)} value={tabLabel(col, i)} className='text-xs'>
              {tabLabel(col, i)}
            </TabsTrigger>
          ))}
        </TabsList>
        {results.map((col, i) => (
          <TabsContent
            key={tabLabel(col, i)}
            value={tabLabel(col, i)}
            className='min-h-0 flex-1 overflow-auto p-4'
          >
            <SingleColumnCard col={col} />
          </TabsContent>
        ))}
      </Tabs>
      <TimingBar item={item} />
    </div>
  );
};

// ── GetMetricDefinitionView ───────────────────────────────────────────────────

export const GetMetricDefinitionView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ metric?: string }>(item.toolInput);
  const metric = input?.metric ?? item.toolName;

  const output = parseToolJson<{
    formula?: string;
    type?: string;
    table?: string;
    description?: string;
    dimension_count?: number;
  }>(item.toolOutput);

  const fields = [
    { label: "Metric", value: metric },
    { label: "Type", value: output?.type ?? "—" },
    { label: "Table", value: output?.table ?? "—" },
    output?.dimension_count !== undefined
      ? { label: "Dimensions", value: String(output.dimension_count) }
      : null
  ].filter(Boolean) as { label: string; value: string }[];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            {fields.map((f) => (
              <div key={f.label} className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                  {f.label}
                </p>
                <p className='font-medium font-mono text-xs'>{f.value}</p>
              </div>
            ))}
          </div>
          {output?.description && (
            <div>
              <p className='mb-1 font-medium text-muted-foreground text-xs'>Description</p>
              <p className='text-xs'>{output.description}</p>
            </div>
          )}
          {output?.formula && (
            <div>
              <p className='mb-1 font-medium text-muted-foreground text-xs'>Formula</p>
              <pre className='whitespace-pre-wrap rounded border bg-muted/50 p-2.5 font-mono text-[11px]'>
                {output.formula}
              </pre>
            </div>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── GetJoinPathView ───────────────────────────────────────────────────────────

export const GetJoinPathView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{
    from_entity?: string;
    to_entity?: string;
    from?: string;
    to?: string;
  }>(item.toolInput);
  // analytics uses from_entity/to_entity; app-builder uses from/to
  const from = input?.from_entity ?? input?.from ?? "?";
  const to = input?.to_entity ?? input?.to ?? "?";

  const output = parseToolJson<{ path?: string; join_type?: string }>(item.toolOutput);

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='flex items-center gap-2 rounded border bg-muted/30 px-3 py-2.5 font-mono text-xs'>
            <span className='font-medium'>{from}</span>
            <span className='text-muted-foreground'>→</span>
            <span className='font-medium'>{to}</span>
            {output?.join_type && (
              <span className='ml-auto shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground'>
                {output.join_type}
              </span>
            )}
          </div>
          {output?.path && (
            <div>
              <p className='mb-1 font-medium text-muted-foreground text-xs'>Join Expression</p>
              <pre className='whitespace-pre-wrap rounded border bg-muted/50 p-2.5 font-mono text-[11px]'>
                {output.path}
              </pre>
            </div>
          )}
          {!output && item.toolOutput && (
            <div>
              <p className='mb-1 font-medium text-muted-foreground text-xs'>Result</p>
              <p className='text-muted-foreground text-xs'>{item.toolOutput}</p>
            </div>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── RenderChartView ───────────────────────────────────────────────────────────

export const RenderChartView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{
    chart_type?: string;
    title?: string;
    x?: string;
    y?: string;
    series?: string;
    name?: string;
    value?: string;
    x_axis_label?: string;
    y_axis_label?: string;
  }>(item.toolInput);

  const output = parseToolJson<{ ok?: boolean; errors?: string[] }>(item.toolOutput);
  const ok = output?.ok ?? item.isStreaming;
  const errors = output?.errors ?? [];

  const chartFields = [
    { label: "Type", value: input?.chart_type ?? "—" },
    input?.title ? { label: "Title", value: input.title } : null,
    input?.x ? { label: "X", value: input.x } : null,
    input?.y ? { label: "Y", value: input.y } : null,
    input?.series ? { label: "Series", value: input.series } : null,
    input?.name ? { label: "Name", value: input.name } : null,
    input?.value ? { label: "Value", value: input.value } : null,
    input?.x_axis_label ? { label: "X Label", value: input.x_axis_label } : null,
    input?.y_axis_label ? { label: "Y Label", value: input.y_axis_label } : null
  ].filter(Boolean) as { label: string; value: string }[];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div
            className={`flex items-center gap-2 rounded border px-3 py-2 text-xs ${
              item.isStreaming
                ? "border-border bg-muted/30 text-muted-foreground"
                : ok
                  ? "border-success/30 bg-success/5 text-success"
                  : "border-destructive/30 bg-destructive/5 text-destructive"
            }`}
          >
            {item.isStreaming ? (
              <Spinner className='size-3' />
            ) : ok ? (
              <span>✓</span>
            ) : (
              <span>✕</span>
            )}
            <span className='font-medium'>
              {item.isStreaming ? "Rendering…" : ok ? "Chart rendered" : "Render failed"}
            </span>
          </div>
          <div className='grid grid-cols-2 gap-2'>
            {chartFields.map((f) => (
              <div key={f.label} className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                  {f.label}
                </p>
                <p className='font-medium font-mono text-xs'>{f.value}</p>
              </div>
            ))}
          </div>
          {errors.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-destructive text-xs'>Errors</p>
              <div className='space-y-1.5'>
                {errors.map((e) => (
                  <p
                    key={e}
                    className='rounded border border-destructive/30 bg-destructive/5 px-2.5 py-1.5 text-[11px] text-destructive'
                  >
                    {e}
                  </p>
                ))}
              </div>
            </div>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── ResolveSchemaView ─────────────────────────────────────────────────────────

export const ResolveSchemaView = ({ item }: { item: ArtifactItem }) => {
  // toolOutput is plain text: "Tables: table1, table2, ..."
  const raw = item.toolOutput ?? "";
  const tablesPart = raw.startsWith("Tables: ") ? raw.slice("Tables: ".length) : raw;
  const tables = tablesPart
    .split(",")
    .map((t) => t.trim())
    .filter(Boolean);

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-3'>
          {tables.length > 0 ? (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
                Tables ({tables.length})
              </p>
              <div className='flex flex-wrap gap-1.5'>
                {tables.map((t) => (
                  <span key={t} className='rounded-full bg-muted px-2.5 py-0.5 font-mono text-xs'>
                    {t}
                  </span>
                ))}
              </div>
            </div>
          ) : (
            <p className='text-muted-foreground text-xs'>{raw || "Schema resolved"}</p>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── AutomationStepView ─────────────────────────────────────────────────────────
//
// Automation steps set toolInput = "Running…" (a plain string, never JSON),
// which reliably identifies them vs. LLM tool calls that always get
// JSON.stringify(input).

export const AutomationStepView = ({ item }: { item: ArtifactItem }) => {
  const isRunning = item.isStreaming;
  const isSuccess = !item.isStreaming && item.toolOutput === "Completed";
  const error = !item.isStreaming && item.toolOutput !== "Completed" ? item.toolOutput : undefined;

  return (
    <div className='flex h-full flex-col gap-4 p-4'>
      <div className='flex items-center gap-3 rounded-lg border bg-muted/30 p-4'>
        {isRunning && (
          <>
            <Spinner className='text-muted-foreground' />
            <span className='text-muted-foreground text-sm'>Running…</span>
          </>
        )}
        {isSuccess && (
          <>
            <span className='flex h-5 w-5 items-center justify-center rounded-full bg-success/15 text-success'>
              ✓
            </span>
            <span className='text-sm text-success'>Completed successfully</span>
          </>
        )}
        {error !== undefined && (
          <>
            <span className='flex h-5 w-5 items-center justify-center rounded-full bg-destructive/15 text-destructive'>
              ✕
            </span>
            <span className='text-destructive text-sm'>Failed</span>
          </>
        )}
      </div>

      {error && (
        <div>
          <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Error</p>
          <ErrorAlert
            message={<pre className='whitespace-pre-wrap font-mono text-xs'>{error}</pre>}
          />
        </div>
      )}
    </div>
  );
};

// ── ColumnValuesView ──────────────────────────────────────────────────────────

export const ColumnValuesView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ table?: string; column?: string }>(item.toolInput);
  const table = input?.table ?? "?";
  const column = input?.column ?? "?";
  const output = parseToolJson<{ values?: unknown[]; error?: string }>(item.toolOutput);
  const values = output?.values ?? [];
  const error = output?.error;

  return (
    <div className='h-full overflow-auto p-4'>
      <div className='space-y-4'>
        <div className='grid grid-cols-2 gap-2'>
          <div className='rounded border bg-muted/30 px-2.5 py-2'>
            <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Table</p>
            <p className='font-medium font-mono text-xs'>{table}</p>
          </div>
          <div className='rounded border bg-muted/30 px-2.5 py-2'>
            <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Column</p>
            <p className='font-medium font-mono text-xs'>{column}</p>
          </div>
        </div>
        {error && <ErrorAlert message={error} />}
        {values.length > 0 && (
          <div>
            <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
              Values ({values.length})
            </p>
            <div className='overflow-hidden rounded border'>
              <table className='w-full text-xs'>
                <thead>
                  <tr className='bg-muted/50'>
                    <th className='px-2.5 py-1.5 text-left font-medium text-muted-foreground'>
                      {column}
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {values.map((v, i) => (
                    // biome-ignore lint/suspicious/noArrayIndexKey: stable ordered list
                    <tr key={i} className={i % 2 === 0 ? "bg-background" : "bg-muted/20"}>
                      <td className='px-2.5 py-1 font-mono'>{v == null ? "null" : toText(v)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        )}
      </div>
    </div>
  );
};

// ── ColumnRangeView ───────────────────────────────────────────────────────────

export const ColumnRangeView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ table?: string; column?: string }>(item.toolInput);
  const table = input?.table ?? "?";
  const column = input?.column ?? "?";
  const output = parseToolJson<{
    min?: string;
    max?: string;
    distinct_count?: string;
    error?: string;
  }>(item.toolOutput);
  const error = output?.error;

  const stats = [
    { label: "Table", value: table },
    { label: "Column", value: column },
    output?.min !== undefined ? { label: "Min", value: output.min } : null,
    output?.max !== undefined ? { label: "Max", value: output.max } : null,
    output?.distinct_count !== undefined
      ? { label: "Distinct", value: output.distinct_count }
      : null
  ].filter(Boolean) as { label: string; value: string }[];

  return (
    <div className='h-full overflow-auto p-4'>
      <div className='space-y-4'>
        <div className='grid grid-cols-2 gap-2'>
          {stats.map((s) => (
            <div key={s.label} className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>{s.label}</p>
              <p className='font-medium font-mono text-xs'>{s.value}</p>
            </div>
          ))}
        </div>
        {error && <ErrorAlert message={error} />}
      </div>
    </div>
  );
};

// ── CompileSemanticQueryView ──────────────────────────────────────────────────

export const CompileSemanticQueryView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{
    measures?: string[];
    dimensions?: string[];
    filters?: unknown[];
    time_dimensions?: unknown[];
  }>(item.toolInput);
  const output = parseToolJson<{ success?: boolean; sql?: string; error?: string }>(
    item.toolOutput
  );
  const success = output?.success !== false;

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          {(input?.measures?.length ?? 0) > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Measures</p>
              <div className='flex flex-wrap gap-1' data-testid='semantic-query-measures'>
                {(input?.measures ?? []).map((m) => (
                  <span
                    key={m}
                    className='rounded border bg-muted/50 px-1.5 py-0.5 font-mono text-[11px]'
                  >
                    {m}
                  </span>
                ))}
              </div>
            </div>
          )}
          {(input?.dimensions?.length ?? 0) > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Dimensions</p>
              <div className='flex flex-wrap gap-1'>
                {(input?.dimensions ?? []).map((d) => (
                  <span
                    key={d}
                    className='rounded border bg-muted/50 px-1.5 py-0.5 font-mono text-[11px]'
                  >
                    {d}
                  </span>
                ))}
              </div>
            </div>
          )}
          {Array.isArray(input?.time_dimensions) && input.time_dimensions.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Time dimensions</p>
              <div className='flex flex-wrap gap-1'>
                {(input.time_dimensions as Record<string, unknown>[]).map((td) => {
                  const dimension = toText(td.dimension);
                  const granularity = td.granularity != null ? toText(td.granularity) : null;
                  return (
                    <span
                      key={`${dimension}-${granularity ?? "none"}`}
                      className='inline-flex items-center gap-1 rounded border bg-muted/50 px-1.5 py-0.5 font-mono text-[11px]'
                    >
                      <span className='font-medium'>{dimension.split(".").pop()}</span>
                      {granularity && (
                        <span className='text-muted-foreground'>by {granularity}</span>
                      )}
                    </span>
                  );
                })}
              </div>
            </div>
          )}
          {Array.isArray(input?.filters) && input.filters.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Filters</p>
              <div className='flex flex-wrap gap-1'>
                {(input.filters as Record<string, unknown>[]).map((f, i) => {
                  // Handle both formats: { field, op, value } and { member, operator, values }
                  const field = toText(f.field ?? f.member);
                  const op = toText(f.op ?? f.operator);
                  const rawVal = f.value ?? f.values;
                  const val = Array.isArray(rawVal)
                    ? rawVal.map((v) => toText(v)).join(", ")
                    : toText(rawVal);
                  return (
                    <span
                      key={`${field}-${op}-${i}`}
                      className='inline-flex items-center gap-1 rounded border bg-muted/50 px-1.5 py-0.5 font-mono text-[11px]'
                    >
                      <span className='font-medium'>{field.split(".").pop()}</span>
                      <span className='text-muted-foreground'>{op}</span>
                      <span>{val}</span>
                    </span>
                  );
                })}
              </div>
            </div>
          )}
          {success && output?.sql && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Compiled SQL</p>
              <pre className='whitespace-pre-wrap rounded border bg-muted/50 p-3 font-mono text-[11px]'>
                {output.sql}
              </pre>
            </div>
          )}
          {!success && output?.error && (
            <p className='rounded border border-destructive/30 bg-destructive/5 px-2.5 py-1.5 text-[11px] text-destructive'>
              {output.error}
            </p>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── TestDbtModelsView ─────────────────────────────────────────────────────────

interface TestResult {
  test_name?: string;
  model_name?: string;
  column_name?: string;
  status?: string;
  failures?: number;
  duration_ms?: number;
  message?: string;
}

export const TestDbtModelsView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ project?: string; selector?: string | null }>(item.toolInput);
  const output = parseToolJson<{
    ok?: boolean;
    tests_run?: number;
    passed?: number;
    failed?: number;
    results?: TestResult[];
    error?: string;
  }>(item.toolOutput);
  const results = output?.results ?? [];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Project</p>
              <p className='font-medium font-mono text-xs'>{input?.project ?? "—"}</p>
            </div>
            {input?.selector && (
              <div className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                  Selector
                </p>
                <p className='font-medium font-mono text-xs'>{input.selector}</p>
              </div>
            )}
            {output?.tests_run !== undefined && (
              <div className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                  Tests Run
                </p>
                <p className='font-medium font-mono text-xs'>{output.tests_run}</p>
              </div>
            )}
            {output?.passed !== undefined && (
              <div className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Passed</p>
                <p className='font-medium font-mono text-success text-xs'>{output.passed}</p>
              </div>
            )}
            {output?.failed !== undefined && (
              <div className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Failed</p>
                <p
                  className={`font-medium font-mono text-xs ${output.failed > 0 ? "text-destructive" : ""}`}
                >
                  {output.failed}
                </p>
              </div>
            )}
          </div>

          {output?.error && <ErrorAlert message={output.error} />}

          {results.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
                Results ({results.length})
              </p>
              <div className='space-y-1.5'>
                {results.map((r, i) => {
                  const isPass = r.status === "PASS";
                  const isFail = r.status === "FAIL" || r.status === "ERROR";
                  return (
                    // biome-ignore lint/suspicious/noArrayIndexKey: stable ordered list
                    <div key={i} className='rounded border bg-muted/30 px-2.5 py-2'>
                      <div className='flex items-start gap-2'>
                        <div className='min-w-0 flex-1'>
                          <p className='font-medium font-mono text-xs'>{r.test_name}</p>
                          {(r.model_name || r.column_name) && (
                            <p className='mt-0.5 font-mono text-[10px] text-muted-foreground'>
                              {[r.model_name, r.column_name].filter(Boolean).join(".")}
                            </p>
                          )}
                        </div>
                        {r.status && (
                          <span
                            className={`shrink-0 rounded px-1.5 py-0.5 font-mono text-[10px] ${
                              isPass
                                ? "bg-success/10 text-success"
                                : isFail
                                  ? "bg-destructive/10 text-destructive"
                                  : "bg-muted text-muted-foreground"
                            }`}
                          >
                            {r.status}
                          </span>
                        )}
                      </div>
                      <div className='mt-0.5 flex gap-3 text-[11px] text-muted-foreground'>
                        {r.failures !== undefined && r.failures > 0 && (
                          <span className='text-destructive'>{r.failures} failures</span>
                        )}
                        {r.duration_ms !== undefined && <span>{r.duration_ms}ms</span>}
                      </div>
                      {r.message && isFail && (
                        <p className='mt-1 text-[11px] text-destructive'>{r.message}</p>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── ListDbtNodesView ──────────────────────────────────────────────────────────

interface DbtNode {
  unique_id?: string;
  name?: string;
  resource_type?: string;
  path?: string;
  materialization?: string;
  description?: string;
}

export const ListDbtNodesView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ project?: string }>(item.toolInput);
  const output = parseToolJson<{
    ok?: boolean;
    count?: number;
    nodes?: DbtNode[];
    error?: string;
  }>(item.toolOutput);
  const nodes = output?.nodes ?? [];

  const byType = nodes.reduce<Record<string, DbtNode[]>>((acc, n) => {
    const t = n.resource_type ?? "other";
    if (!acc[t]) acc[t] = [];
    acc[t].push(n);
    return acc;
  }, {});
  const types = Object.keys(byType).sort();
  const [activeType, setActiveType] = useState(types[0] ?? null);

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Project</p>
              <p className='font-medium font-mono text-xs'>{input?.project ?? "—"}</p>
            </div>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Total</p>
              <p className='font-medium font-mono text-xs'>{output?.count ?? nodes.length}</p>
            </div>
          </div>

          {output?.error && <ErrorAlert message={output.error} />}

          {types.length > 0 && (
            <Tabs value={activeType ?? undefined} onValueChange={setActiveType}>
              <TabsList variant='line' className='overflow-x-auto border-b'>
                {types.map((t) => (
                  <TabsTrigger key={t} value={t} className='text-xs'>
                    {t}
                    <span className='ml-1 rounded bg-muted px-1 py-0.5 text-[10px]'>
                      {byType[t].length}
                    </span>
                  </TabsTrigger>
                ))}
              </TabsList>
              {types.map((t) => (
                <TabsContent key={t} value={t} className='pt-2'>
                  <div className='space-y-1'>
                    {byType[t].map((n) => (
                      <div
                        key={n.unique_id ?? n.name}
                        className='rounded border bg-muted/30 px-2.5 py-2'
                      >
                        <div className='flex items-center gap-2'>
                          <span className='font-medium font-mono text-xs'>{n.name}</span>
                          {n.materialization && (
                            <span className='ml-auto rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground'>
                              {n.materialization}
                            </span>
                          )}
                        </div>
                        {n.path && (
                          <p className='mt-0.5 break-all font-mono text-[10px] text-muted-foreground'>
                            {n.path}
                          </p>
                        )}
                        {n.description && (
                          <p className='mt-0.5 text-[11px] text-muted-foreground'>
                            {n.description}
                          </p>
                        )}
                      </div>
                    ))}
                  </div>
                </TabsContent>
              ))}
            </Tabs>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── GetDbtLineageView ─────────────────────────────────────────────────────────

interface LineageNode {
  unique_id?: string;
  name?: string;
  resource_type?: string;
  description?: string;
  path?: string;
}

interface LineageEdge {
  source?: string;
  target?: string;
}

export const GetDbtLineageView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ project?: string }>(item.toolInput);
  const output = parseToolJson<{
    ok?: boolean;
    nodes?: LineageNode[];
    edges?: LineageEdge[];
    error?: string;
  }>(item.toolOutput);
  const nodes = output?.nodes ?? [];
  const edges = output?.edges ?? [];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Project</p>
              <p className='font-medium font-mono text-xs'>{input?.project ?? "—"}</p>
            </div>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Nodes</p>
              <p className='font-medium font-mono text-xs'>{nodes.length}</p>
            </div>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Edges</p>
              <p className='font-medium font-mono text-xs'>{edges.length}</p>
            </div>
          </div>

          {output?.error && <ErrorAlert message={output.error} />}

          {nodes.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
                Nodes ({nodes.length})
              </p>
              <div className='space-y-1'>
                {nodes.map((n) => (
                  <div
                    key={n.unique_id ?? n.name}
                    className='flex items-center gap-2 rounded border bg-muted/30 px-2.5 py-1.5'
                  >
                    <span className='font-medium font-mono text-xs'>{n.name}</span>
                    {n.resource_type && (
                      <span className='ml-auto rounded bg-muted px-1.5 py-0.5 font-mono text-[10px] text-muted-foreground'>
                        {n.resource_type}
                      </span>
                    )}
                  </div>
                ))}
              </div>
            </div>
          )}

          {edges.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
                Edges ({edges.length})
              </p>
              <div className='overflow-hidden rounded border'>
                <table className='w-full text-xs'>
                  <thead>
                    <tr className='bg-muted/50'>
                      <th className='px-2.5 py-1.5 text-left font-medium text-muted-foreground'>
                        Source
                      </th>
                      <th className='px-2.5 py-1.5 text-left font-medium text-muted-foreground'>
                        Target
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {edges.map((e, i) => (
                      // biome-ignore lint/suspicious/noArrayIndexKey: stable ordered list
                      <tr key={i} className={i % 2 === 0 ? "bg-background" : "bg-muted/20"}>
                        <td className='px-2.5 py-1 font-mono'>{e.source}</td>
                        <td className='px-2.5 py-1 font-mono'>{e.target}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </div>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── CompileDbtModelView ───────────────────────────────────────────────────────

interface CompiledNode {
  name?: string;
  unique_id?: string;
  compiled_sql?: string;
}

interface CompileError {
  node_id?: string;
  message?: string;
}

export const CompileDbtModelView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ project?: string; model?: string }>(item.toolInput);
  const isSingle = !!input?.model;

  const output = parseToolJson<{
    ok?: boolean;
    // single-model shape
    model?: string;
    compiled_sql?: string;
    // all-models shape
    models_compiled?: number;
    errors?: CompileError[];
    nodes?: CompiledNode[];
    error?: string;
  }>(item.toolOutput);

  const errors = output?.errors ?? [];
  const nodes = output?.nodes ?? [];
  const [nodeTab, setNodeTab] = useState(nodes[0]?.name ?? nodes[0]?.unique_id ?? null);

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Project</p>
              <p className='font-medium font-mono text-xs'>{input?.project ?? "—"}</p>
            </div>
            {isSingle ? (
              <div className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Model</p>
                <p className='font-medium font-mono text-xs'>{input?.model}</p>
              </div>
            ) : (
              output?.models_compiled !== undefined && (
                <div className='rounded border bg-muted/30 px-2.5 py-2'>
                  <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                    Compiled
                  </p>
                  <p className='font-medium font-mono text-xs'>{output.models_compiled}</p>
                </div>
              )
            )}
          </div>

          {output?.error && <ErrorAlert message={output.error} />}

          {errors.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-destructive text-xs'>
                Errors ({errors.length})
              </p>
              <div className='space-y-1'>
                {errors.map((e, i) => (
                  // biome-ignore lint/suspicious/noArrayIndexKey: stable ordered list
                  <div
                    key={i}
                    className='rounded border border-destructive/30 bg-destructive/5 px-2.5 py-1.5'
                  >
                    {e.node_id && (
                      <p className='mb-0.5 font-medium font-mono text-[11px]'>{e.node_id}</p>
                    )}
                    <p className='text-[11px] text-destructive'>{e.message}</p>
                  </div>
                ))}
              </div>
            </div>
          )}

          {/* Single model: show compiled SQL directly */}
          {isSingle && output?.compiled_sql && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Compiled SQL</p>
              <pre className='overflow-auto whitespace-pre-wrap rounded border bg-muted/50 p-3 font-mono text-[11px]'>
                {output.compiled_sql}
              </pre>
            </div>
          )}

          {/* All models: tabbed SQL per node */}
          {!isSingle && nodes.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
                Models ({nodes.length})
              </p>
              <Tabs value={nodeTab ?? undefined} onValueChange={setNodeTab}>
                <div className='overflow-x-auto border-b'>
                  <TabsList variant='line'>
                    {nodes.map((n) => {
                      const key = n.name ?? n.unique_id ?? "";
                      return (
                        <TabsTrigger key={key} value={key} className='text-xs'>
                          {n.name ?? n.unique_id}
                        </TabsTrigger>
                      );
                    })}
                  </TabsList>
                </div>
                {nodes.map((n) => {
                  const key = n.name ?? n.unique_id ?? "";
                  return (
                    <TabsContent key={key} value={key} className='pt-2'>
                      {n.compiled_sql ? (
                        <pre className='overflow-auto whitespace-pre-wrap rounded border bg-muted/50 p-3 font-mono text-[11px]'>
                          {n.compiled_sql}
                        </pre>
                      ) : (
                        <p className='text-muted-foreground text-xs'>No SQL available.</p>
                      )}
                    </TabsContent>
                  );
                })}
              </Tabs>
            </div>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── RunDbtModelsView ──────────────────────────────────────────────────────────

export const RunDbtModelsView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ project?: string; selector?: string }>(item.toolInput);
  const output = parseToolJson<{
    ok?: boolean;
    status?: string;
    duration_ms?: number;
    results?: Array<{
      unique_id?: string;
      name?: string;
      status?: string;
      duration_ms?: number;
      rows_affected?: number;
      message?: string;
    }>;
    error?: string;
  }>(item.toolOutput);

  const results = output?.results ?? [];
  const successCount = results.filter(
    (r) => r.status === "SUCCESS" || r.status === "success"
  ).length;
  const errorCount = results.filter((r) => r.status === "ERROR" || r.status === "error").length;
  const skipCount = results.filter((r) => r.status === "SKIP" || r.status === "skip").length;

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Project</p>
              <p className='font-medium font-mono text-xs'>{input?.project ?? "—"}</p>
            </div>
            {input?.selector && (
              <div className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                  Selector
                </p>
                <p className='font-medium font-mono text-xs'>{input.selector}</p>
              </div>
            )}
            {output?.duration_ms !== undefined && (
              <div className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                  Duration
                </p>
                <p className='font-medium font-mono text-xs'>{output.duration_ms}ms</p>
              </div>
            )}
            {results.length > 0 && (
              <div className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Models</p>
                <p className='font-medium font-mono text-xs'>
                  {successCount > 0 && <span className='text-success'>{successCount} ok</span>}
                  {errorCount > 0 && (
                    <span className={successCount > 0 ? "· text-destructive" : "text-destructive"}>
                      {errorCount} err
                    </span>
                  )}
                  {skipCount > 0 && (
                    <span className='text-muted-foreground'> · {skipCount} skip</span>
                  )}
                </p>
              </div>
            )}
          </div>

          {output?.error && <ErrorAlert message={output.error} />}

          {results.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
                Results ({results.length})
              </p>
              <div className='space-y-1.5'>
                {results.map((r, i) => {
                  const isOk = r.status === "SUCCESS" || r.status === "success";
                  const isSkip = r.status === "SKIP" || r.status === "skip";
                  return (
                    // biome-ignore lint/suspicious/noArrayIndexKey: stable ordered list
                    <div key={i} className='rounded border bg-muted/30 px-2.5 py-2'>
                      <div className='flex items-center gap-2'>
                        <span className='font-medium font-mono text-xs'>
                          {r.name ?? r.unique_id}
                        </span>
                        {r.status && (
                          <span
                            className={`ml-auto rounded px-1.5 py-0.5 font-mono text-[10px] ${
                              isOk
                                ? "bg-success/10 text-success"
                                : isSkip
                                  ? "bg-muted text-muted-foreground"
                                  : "bg-destructive/10 text-destructive"
                            }`}
                          >
                            {r.status}
                          </span>
                        )}
                      </div>
                      <div className='mt-0.5 flex gap-3 text-[11px] text-muted-foreground'>
                        {r.rows_affected != null && (
                          <span>{r.rows_affected.toLocaleString()} rows</span>
                        )}
                        {r.duration_ms !== undefined && <span>{r.duration_ms}ms</span>}
                      </div>
                      {r.message && !isOk && !isSkip && (
                        <p className='mt-1 text-[11px] text-destructive'>{r.message}</p>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── AnalyzeDbtProjectView ─────────────────────────────────────────────────────

interface DbtDiagnostic {
  kind?: string;
  message?: string;
}

interface DbtContractViolation {
  model?: string;
  kind?: string;
  message?: string;
}

interface DbtSchema {
  name?: string;
  columns?: Array<{ name?: string; data_type?: string; nullable?: boolean }>;
}

export const AnalyzeDbtProjectView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ project?: string }>(item.toolInput);
  const output = parseToolJson<{
    ok?: boolean;
    models_analyzed?: number;
    cached_count?: number;
    diagnostics?: DbtDiagnostic[];
    contract_violations?: DbtContractViolation[];
    schemas?: DbtSchema[];
    error?: string;
  }>(item.toolOutput);

  const diagnostics = output?.diagnostics ?? [];
  const violations = output?.contract_violations ?? [];
  const schemas = output?.schemas ?? [];
  const defaultTab = schemas.length > 0 ? (schemas[0].name ?? "0") : null;
  const [schemaTab, setSchemaTab] = useState(defaultTab);

  const stats = [
    { label: "Project", value: input?.project ?? "—" },
    output?.models_analyzed !== undefined
      ? { label: "Models Analyzed", value: String(output.models_analyzed) }
      : null,
    output?.cached_count !== undefined
      ? { label: "Cached", value: String(output.cached_count) }
      : null,
    { label: "Diagnostics", value: String(diagnostics.length) },
    { label: "Violations", value: String(violations.length) }
  ].filter(Boolean) as { label: string; value: string }[];

  const activeSchema = schemas.find((s, i) => (s.name ?? String(i)) === schemaTab) ?? schemas[0];

  return (
    <div className='flex h-full flex-col'>
      {/* Fixed top: stats + errors + diagnostics + violations */}
      <div className='shrink-0 space-y-3 p-4'>
        <div className='grid grid-cols-2 gap-2'>
          {stats.map((s) => (
            <div key={s.label} className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>{s.label}</p>
              <p className='font-medium font-mono text-xs'>{s.value}</p>
            </div>
          ))}
        </div>

        {output?.error && <ErrorAlert message={output.error} />}

        {diagnostics.length > 0 && (
          <div>
            <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
              Diagnostics ({diagnostics.length})
            </p>
            <div className='max-h-32 space-y-1 overflow-y-auto'>
              {diagnostics.map((d, i) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: stable ordered list
                <div key={i} className='rounded border bg-muted/30 px-2.5 py-1.5'>
                  {d.kind && (
                    <span className='mr-1.5 rounded bg-muted px-1 py-0.5 font-mono text-[10px] text-muted-foreground'>
                      {d.kind}
                    </span>
                  )}
                  <span className='text-xs'>{d.message}</span>
                </div>
              ))}
            </div>
          </div>
        )}

        {violations.length > 0 && (
          <div>
            <p className='mb-1.5 font-medium text-destructive text-xs'>
              Contract Violations ({violations.length})
            </p>
            <div className='max-h-32 space-y-1 overflow-y-auto'>
              {violations.map((v, i) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: stable ordered list
                <div
                  key={i}
                  className='rounded border border-destructive/30 bg-destructive/5 px-2.5 py-1.5'
                >
                  {v.model && <p className='mb-0.5 font-medium font-mono text-[11px]'>{v.model}</p>}
                  <p className='text-[11px] text-destructive'>{v.message}</p>
                </div>
              ))}
            </div>
          </div>
        )}

        {schemas.length > 0 && (
          <p className='font-medium text-muted-foreground text-xs'>Schemas ({schemas.length})</p>
        )}
      </div>

      {/* Schemas: fills remaining height, tab bar fixed + table scrolls */}
      {schemas.length > 0 && (
        <div className='flex min-h-0 flex-1 flex-col border-t'>
          <div className='shrink-0 overflow-x-auto border-b'>
            <div className='flex'>
              {schemas.map((s, i) => {
                const key = s.name ?? String(i);
                const active = key === schemaTab;
                return (
                  <button
                    key={key}
                    type='button'
                    onClick={() => setSchemaTab(key)}
                    className={`shrink-0 px-3 py-1.5 text-xs transition-colors ${active ? "border-primary border-b-2 font-medium text-foreground" : "text-muted-foreground hover:text-foreground"}`}
                  >
                    {s.name ?? `schema-${i}`}
                  </button>
                );
              })}
            </div>
          </div>
          <div className='flex-1 overflow-auto'>
            {activeSchema && (
              <table className='w-full text-xs'>
                <thead className='sticky top-0 z-10'>
                  <tr className='bg-muted/50'>
                    <th className='px-2.5 py-1.5 text-left font-medium text-muted-foreground'>
                      Column
                    </th>
                    <th className='px-2.5 py-1.5 text-left font-medium text-muted-foreground'>
                      Type
                    </th>
                    <th className='px-2.5 py-1.5 text-left font-medium text-muted-foreground'>
                      Nullable
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {(activeSchema.columns ?? []).map((c, ci) => (
                    // biome-ignore lint/suspicious/noArrayIndexKey: stable ordered list
                    <tr key={ci} className={ci % 2 === 0 ? "bg-background" : "bg-muted/20"}>
                      <td className='px-2.5 py-1 font-mono'>{c.name}</td>
                      <td className='px-2.5 py-1 text-muted-foreground'>{c.data_type ?? "—"}</td>
                      <td className='px-2.5 py-1 text-muted-foreground'>
                        {c.nullable === true ? "yes" : c.nullable === false ? "no" : "—"}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
          </div>
        </div>
      )}

      <TimingBar item={item} />
    </div>
  );
};

// ── GetDbtColumnLineageView ────────────────────────────────────────────────────

interface ColumnLineageEdge {
  source_node?: string;
  source_column?: string;
  target_node?: string;
  target_column?: string;
  dependency_type?: string;
}

export const GetDbtColumnLineageView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ project?: string }>(item.toolInput);
  const output = parseToolJson<{ ok?: boolean; edges?: ColumnLineageEdge[]; error?: string }>(
    item.toolOutput
  );
  const edges = output?.edges ?? [];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Project</p>
              <p className='font-medium font-mono text-xs'>{input?.project ?? "—"}</p>
            </div>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Edges</p>
              <p className='font-medium font-mono text-xs'>{edges.length}</p>
            </div>
          </div>

          {output?.error && <ErrorAlert message={output.error} />}

          {edges.length > 0 ? (
            <div className='overflow-hidden rounded border'>
              <table className='w-full text-xs'>
                <thead>
                  <tr className='bg-muted/50'>
                    <th className='px-2.5 py-1.5 text-left font-medium text-muted-foreground'>
                      Source
                    </th>
                    <th className='px-2.5 py-1.5 text-left font-medium text-muted-foreground'>
                      Target
                    </th>
                    <th className='px-2.5 py-1.5 text-left font-medium text-muted-foreground'>
                      Type
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {edges.map((e, i) => (
                    // biome-ignore lint/suspicious/noArrayIndexKey: stable ordered list
                    <tr key={i} className={i % 2 === 0 ? "bg-background" : "bg-muted/20"}>
                      <td className='px-2.5 py-1 font-mono'>
                        {e.source_node}.{e.source_column}
                      </td>
                      <td className='px-2.5 py-1 font-mono'>
                        {e.target_node}.{e.target_column}
                      </td>
                      <td className='px-2.5 py-1 text-muted-foreground'>
                        {e.dependency_type ?? "—"}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            item.toolOutput && (
              <p className='text-muted-foreground text-xs'>No lineage edges found.</p>
            )
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── ParseDbtProjectView ───────────────────────────────────────────────────────

export const ParseDbtProjectView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ project?: string }>(item.toolInput);
  const output = parseToolJson<{
    ok?: boolean;
    models?: number;
    seeds?: number;
    snapshots?: number;
    tests?: number;
    sources?: number;
    nodes?: number;
    edges?: number;
    duration_ms?: number;
    error?: string;
  }>(item.toolOutput);

  const stats = [
    { label: "Project", value: input?.project ?? "—" },
    output?.models !== undefined ? { label: "Models", value: String(output.models) } : null,
    output?.seeds !== undefined ? { label: "Seeds", value: String(output.seeds) } : null,
    output?.snapshots !== undefined
      ? { label: "Snapshots", value: String(output.snapshots) }
      : null,
    output?.tests !== undefined ? { label: "Tests", value: String(output.tests) } : null,
    output?.sources !== undefined ? { label: "Sources", value: String(output.sources) } : null,
    output?.nodes !== undefined ? { label: "DAG Nodes", value: String(output.nodes) } : null,
    output?.edges !== undefined ? { label: "DAG Edges", value: String(output.edges) } : null,
    output?.duration_ms !== undefined
      ? { label: "Duration", value: `${output.duration_ms}ms` }
      : null
  ].filter(Boolean) as { label: string; value: string }[];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            {stats.map((s) => (
              <div key={s.label} className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                  {s.label}
                </p>
                <p className='font-medium font-mono text-xs'>{s.value}</p>
              </div>
            ))}
          </div>
          {output?.error && <ErrorAlert message={output.error} />}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── SeedDbtProjectView ────────────────────────────────────────────────────────

interface SeedResult {
  unique_id?: string;
  name?: string;
  status?: string;
  duration_ms?: number;
  rows_affected?: number;
  message?: string;
}

export const SeedDbtProjectView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ project?: string }>(item.toolInput);
  const output = parseToolJson<{
    ok?: boolean;
    seeds_loaded?: number;
    results?: SeedResult[];
    error?: string;
  }>(item.toolOutput);
  const results = output?.results ?? [];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Project</p>
              <p className='font-medium font-mono text-xs'>{input?.project ?? "—"}</p>
            </div>
            {output?.seeds_loaded !== undefined && (
              <div className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                  Seeds Loaded
                </p>
                <p className='font-medium font-mono text-xs'>{output.seeds_loaded}</p>
              </div>
            )}
          </div>

          {output?.error && <ErrorAlert message={output.error} />}

          {results.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
                Results ({results.length})
              </p>
              <div className='space-y-1.5'>
                {results.map((r, i) => {
                  const isOk =
                    r.status === "SUCCESS" || r.status === "success" || r.status === "ok";
                  return (
                    // biome-ignore lint/suspicious/noArrayIndexKey: stable ordered list
                    <div key={i} className='rounded border bg-muted/30 px-2.5 py-2'>
                      <div className='flex items-center gap-2'>
                        <span
                          className={
                            isOk
                              ? "font-medium font-mono text-xs"
                              : "font-medium font-mono text-destructive text-xs"
                          }
                        >
                          {r.name ?? r.unique_id}
                        </span>
                        {r.status && (
                          <span
                            className={`ml-auto rounded px-1.5 py-0.5 font-mono text-[10px] ${
                              isOk
                                ? "bg-success/10 text-success"
                                : "bg-destructive/10 text-destructive"
                            }`}
                          >
                            {r.status}
                          </span>
                        )}
                      </div>
                      <div className='mt-0.5 flex gap-3 text-[11px] text-muted-foreground'>
                        {r.rows_affected != null && (
                          <span>{r.rows_affected.toLocaleString()} rows</span>
                        )}
                        {r.duration_ms !== undefined && <span>{r.duration_ms}ms</span>}
                      </div>
                      {r.message && !isOk && (
                        <p className='mt-1 text-[11px] text-destructive'>{r.message}</p>
                      )}
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── DebugDbtProjectView ───────────────────────────────────────────────────────

export const DebugDbtProjectView = ({ item }: { item: ArtifactItem }) => {
  const output = parseToolJson<{
    ok?: boolean;
    project_name?: string;
    version?: string;
    profile?: string;
    has_profiles_yml?: boolean;
    model_paths?: string[];
    seed_paths?: string[];
    model_count?: number;
    seed_count?: number;
    source_count?: number;
    all_ok?: boolean;
    issues?: string[];
    error?: string;
  }>(item.toolOutput);

  const stats = [
    output?.project_name ? { label: "Project", value: output.project_name } : null,
    output?.version ? { label: "Version", value: output.version } : null,
    output?.profile ? { label: "Profile", value: output.profile } : null,
    output?.has_profiles_yml !== undefined
      ? { label: "profiles.yml", value: output.has_profiles_yml ? "Found" : "Missing" }
      : null,
    output?.model_count !== undefined
      ? { label: "Models", value: String(output.model_count) }
      : null,
    output?.seed_count !== undefined ? { label: "Seeds", value: String(output.seed_count) } : null,
    output?.source_count !== undefined
      ? { label: "Sources", value: String(output.source_count) }
      : null
  ].filter(Boolean) as { label: string; value: string }[];

  const issues = output?.issues ?? [];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          {output?.all_ok !== undefined && (
            <div
              className={`flex items-center gap-2 rounded border px-3 py-2 text-xs ${
                output.all_ok
                  ? "border-success/30 bg-success/5 text-success"
                  : "border-destructive/30 bg-destructive/5 text-destructive"
              }`}
            >
              <span>{output.all_ok ? "✓" : "✕"}</span>
              <span className='font-medium'>
                {output.all_ok ? "All checks passed" : "Issues found"}
              </span>
            </div>
          )}

          <div className='grid grid-cols-2 gap-2'>
            {stats.map((s) => (
              <div key={s.label} className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                  {s.label}
                </p>
                <p className='font-medium font-mono text-xs'>{s.value}</p>
              </div>
            ))}
          </div>

          {output?.model_paths && output.model_paths.length > 0 && (
            <div>
              <p className='mb-1 font-medium text-muted-foreground text-xs'>Model Paths</p>
              <div className='flex flex-wrap gap-1'>
                {output.model_paths.map((p) => (
                  <span key={p} className='rounded bg-muted px-2 py-0.5 font-mono text-xs'>
                    {p}
                  </span>
                ))}
              </div>
            </div>
          )}

          {issues.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-destructive text-xs'>
                Issues ({issues.length})
              </p>
              <div className='space-y-1'>
                {issues.map((issue, i) => (
                  // biome-ignore lint/suspicious/noArrayIndexKey: stable ordered list
                  <p
                    key={i}
                    className='rounded border border-destructive/30 bg-destructive/5 px-2.5 py-1.5 text-[11px] text-destructive'
                  >
                    {issue}
                  </p>
                ))}
              </div>
            </div>
          )}

          {output?.error && <ErrorAlert message={output.error} />}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── CleanDbtProjectView ───────────────────────────────────────────────────────

export const CleanDbtProjectView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ project?: string }>(item.toolInput);
  const output = parseToolJson<{ ok?: boolean; cleaned?: string[]; error?: string }>(
    item.toolOutput
  );
  const cleaned = output?.cleaned ?? [];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Project</p>
              <p className='font-medium font-mono text-xs'>{input?.project ?? "—"}</p>
            </div>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Removed</p>
              <p className='font-medium font-mono text-xs'>{cleaned.length}</p>
            </div>
          </div>

          {output?.error && <ErrorAlert message={output.error} />}

          {cleaned.length > 0 ? (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>Cleaned Paths</p>
              <div className='space-y-1'>
                {cleaned.map((p) => (
                  <p key={p} className='rounded bg-muted/30 px-2.5 py-1.5 font-mono text-xs'>
                    {p}
                  </p>
                ))}
              </div>
            </div>
          ) : (
            item.toolOutput && <p className='text-muted-foreground text-xs'>Nothing to clean.</p>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── DocsGenerateDbtView ───────────────────────────────────────────────────────

export const DocsGenerateDbtView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ project?: string }>(item.toolInput);
  const output = parseToolJson<{
    ok?: boolean;
    manifest_path?: string;
    nodes?: number;
    sources?: number;
    error?: string;
  }>(item.toolOutput);

  const stats = [
    { label: "Project", value: input?.project ?? "—" },
    output?.nodes !== undefined ? { label: "Nodes", value: String(output.nodes) } : null,
    output?.sources !== undefined ? { label: "Sources", value: String(output.sources) } : null
  ].filter(Boolean) as { label: string; value: string }[];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            {stats.map((s) => (
              <div key={s.label} className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                  {s.label}
                </p>
                <p className='font-medium font-mono text-xs'>{s.value}</p>
              </div>
            ))}
          </div>
          {output?.manifest_path && (
            <div>
              <p className='mb-1 font-medium text-muted-foreground text-xs'>Manifest Path</p>
              <p className='break-all rounded bg-muted/30 px-2.5 py-1.5 font-mono text-xs'>
                {output.manifest_path}
              </p>
            </div>
          )}
          {output?.error && <ErrorAlert message={output.error} />}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── FormatDbtSqlView ──────────────────────────────────────────────────────────

export const FormatDbtSqlView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ project?: string; check?: boolean }>(item.toolInput);
  const output = parseToolJson<{
    ok?: boolean;
    files_checked?: number;
    files_changed?: number;
    files?: string[];
    error?: string;
  }>(item.toolOutput);
  const files = output?.files ?? [];
  const isCheckMode = input?.check === true;

  const stats = [
    { label: "Project", value: input?.project ?? "—" },
    { label: "Mode", value: isCheckMode ? "Check only" : "Format in place" },
    output?.files_checked !== undefined
      ? { label: "Files Checked", value: String(output.files_checked) }
      : null,
    output?.files_changed !== undefined
      ? { label: "Files Changed", value: String(output.files_changed) }
      : null
  ].filter(Boolean) as { label: string; value: string }[];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            {stats.map((s) => (
              <div key={s.label} className='rounded border bg-muted/30 px-2.5 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                  {s.label}
                </p>
                <p className='font-medium font-mono text-xs'>{s.value}</p>
              </div>
            ))}
          </div>

          {output?.error && <ErrorAlert message={output.error} />}

          {files.length > 0 && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
                {isCheckMode ? "Would change" : "Changed"} ({files.length})
              </p>
              <div className='space-y-1'>
                {files.map((f) => (
                  <p
                    key={f}
                    className='break-all rounded bg-muted/30 px-2.5 py-1.5 font-mono text-xs'
                  >
                    {f}
                  </p>
                ))}
              </div>
            </div>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── InitDbtProjectView ────────────────────────────────────────────────────────

const SCAFFOLD_FILES = [
  { file: "dbt_project.yml", description: "dbt project configuration" },
  { file: "profiles.yml", description: "Connection profiles (DuckDB)" },
  { file: "README.md", description: "Project README" }
];

export const InitDbtProjectView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ name?: string }>(item.toolInput);
  const output = parseToolJson<{
    ok?: boolean;
    rejected?: boolean;
    project_name?: string;
    project_dir?: string;
    error?: string;
  }>(item.toolOutput);

  const projectName = output?.project_name ?? input?.name;
  const projectDir = output?.project_dir;

  const stats = [
    projectName ? { label: "Project", value: projectName } : null,
    projectDir ? { label: "Directory", value: projectDir } : null
  ].filter(Boolean) as { label: string; value: string }[];

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          {output?.ok !== undefined && (
            <div
              className={`flex items-center gap-2 rounded border px-3 py-2 text-xs ${
                output.ok
                  ? "border-success/30 bg-success/5 text-success"
                  : output.rejected
                    ? "border-muted-foreground/30 bg-muted/30 text-muted-foreground"
                    : "border-destructive/30 bg-destructive/5 text-destructive"
              }`}
            >
              <span>{output.ok ? "✓" : "✕"}</span>
              <span className='font-medium'>
                {output.ok
                  ? "dbt project created"
                  : output.rejected
                    ? "Cancelled"
                    : "Failed to create dbt project"}
              </span>
            </div>
          )}

          {stats.length > 0 && (
            <div className='grid grid-cols-1 gap-2'>
              {stats.map((s) => (
                <div key={s.label} className='rounded border bg-muted/30 px-2.5 py-2'>
                  <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                    {s.label}
                  </p>
                  <p className='break-all font-medium font-mono text-xs'>{s.value}</p>
                </div>
              ))}
            </div>
          )}

          {output?.ok && (
            <div>
              <p className='mb-1.5 font-medium text-muted-foreground text-xs'>
                Files created ({SCAFFOLD_FILES.length})
              </p>
              <div className='space-y-1'>
                {SCAFFOLD_FILES.map(({ file, description }) => (
                  <div
                    key={file}
                    className='flex items-center justify-between rounded bg-muted/30 px-2.5 py-1.5'
                  >
                    <span className='font-mono text-xs'>{file}</span>
                    <span className='text-[10px] text-muted-foreground'>{description}</span>
                  </div>
                ))}
              </div>
            </div>
          )}

          {output?.error && <ErrorAlert message={output.error} />}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── CountRowsView ─────────────────────────────────────────────────────────────

export const CountRowsView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ table?: string; filter?: string | null }>(item.toolInput);
  const table = input?.table ?? "?";
  const filter = typeof input?.filter === "string" ? input.filter : null;
  const output = parseToolJson<{ count?: string; error?: string }>(item.toolOutput);
  const count = output?.count;
  const error = output?.error;

  return (
    <div className='h-full overflow-auto p-4'>
      <div className='space-y-4'>
        <div className='grid grid-cols-2 gap-2'>
          <div className='rounded border bg-muted/30 px-2.5 py-2'>
            <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Table</p>
            <p className='font-medium font-mono text-xs'>{table}</p>
          </div>
          {count !== undefined && (
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Count</p>
              <p className='font-medium font-mono text-xs'>
                {Number.isNaN(Number(count)) ? count : Number(count).toLocaleString()}
              </p>
            </div>
          )}
        </div>
        {filter && (
          <div>
            <p className='mb-1 font-medium text-muted-foreground text-xs'>Filter</p>
            <pre className='whitespace-pre-wrap rounded border bg-muted/50 p-2.5 font-mono text-[11px]'>
              {filter}
            </pre>
          </div>
        )}
        {error && <ErrorAlert message={error} />}
      </div>
    </div>
  );
};

// ── ExplainMetricView ─────────────────────────────────────────────────────────

function fmtNumber(n: number): string {
  if (!Number.isFinite(n)) return String(n);
  const abs = Math.abs(n);
  if (abs >= 1000) return n.toLocaleString(undefined, { maximumFractionDigits: 0 });
  if (abs >= 1) return n.toLocaleString(undefined, { maximumFractionDigits: 2 });
  return n.toLocaleString(undefined, { maximumFractionDigits: 4 });
}

function ToolErrorOrLoading({ item }: { item: ArtifactItem }) {
  if (item.isStreaming) {
    return (
      <div className='flex items-center gap-2 px-4 py-6 text-muted-foreground text-xs'>
        <Spinner className='h-3 w-3' />
        Running…
      </div>
    );
  }
  return null;
}

export const ExplainMetricView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{
    target?: string;
    time_dimension?: string;
    current_period_start?: string;
    current_period_end?: string;
    previous_period_start?: string;
    previous_period_end?: string;
    deep?: boolean;
  }>(item.toolInput);
  const output = parseToolJson<ExplainResult & { error?: string }>(item.toolOutput);
  const error = output?.error;
  const result = output && !error && output.target ? (output as ExplainResult) : null;

  return (
    <div className='flex h-full flex-col'>
      {/* Fixed-height header: input cards + error/loading state */}
      <div className='flex-none p-4 pb-2'>
        <div className='space-y-3'>
          <div className='grid grid-cols-2 gap-2'>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Target</p>
              <p className='font-medium font-mono text-xs'>{input?.target ?? "—"}</p>
            </div>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                Time dimension
              </p>
              <p className='font-medium font-mono text-xs'>{input?.time_dimension ?? "—"}</p>
            </div>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                Current period
              </p>
              <p className='font-medium font-mono text-xs'>
                {input?.current_period_start ?? "—"} → {input?.current_period_end ?? "—"}
              </p>
            </div>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                Previous period
              </p>
              <p className='font-medium font-mono text-xs'>
                {input?.previous_period_start ?? "—"} → {input?.previous_period_end ?? "—"}
              </p>
            </div>
          </div>
          {error && <ErrorAlert message={error} />}
          <ToolErrorOrLoading item={item} />
        </div>
      </div>
      {/* ExplainBody fills remaining space as a proper flex child */}
      <div className='flex min-h-0 flex-1 flex-col px-4 pb-2'>
        {result && <ExplainBody result={result} />}
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── FindOpportunitiesView ────────────────────────────────────────────────────

export const FindOpportunitiesView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{
    target?: string;
    time_dimension?: string;
    period_start?: string;
    period_end?: string;
  }>(item.toolInput);
  const output = parseToolJson<OpportunityResult & { error?: string }>(item.toolOutput);
  const error = output?.error;
  const result = output && !error && output.target ? (output as OpportunityResult) : null;

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='grid grid-cols-2 gap-2'>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Target</p>
              <p className='font-medium font-mono text-xs'>{input?.target ?? "—"}</p>
            </div>
            <div className='rounded border bg-muted/30 px-2.5 py-2'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Period</p>
              <p className='font-medium font-mono text-xs'>
                {input?.period_start ?? "—"} → {input?.period_end ?? "—"}
              </p>
            </div>
          </div>
          {error && <ErrorAlert message={error} />}
          <ToolErrorOrLoading item={item} />
          {result && (
            <>
              <div className='rounded border border-primary/20 bg-primary/5 px-3 py-2'>
                <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                  Overall value
                </p>
                <p className='font-medium font-mono text-sm'>{fmtNumber(result.overall_value)}</p>
                <p className='mt-0.5 text-[10px] text-muted-foreground'>
                  Weight basis: {result.weight_basis}
                </p>
              </div>

              {(result.dimensions ?? []).length === 0 ? (
                <p className='text-muted-foreground text-xs'>
                  No actionable opportunities found in the analysed dimensions.
                </p>
              ) : (
                <div className='space-y-3'>
                  <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                    Opportunities by dimension
                  </p>
                  {(result.dimensions ?? []).map((dim) => (
                    <div key={dim.dimension} className='space-y-1.5'>
                      <div className='flex items-center justify-between gap-2'>
                        <p className='font-medium font-mono text-xs'>{dim.dimension}</p>
                        <div className='flex shrink-0 items-center gap-1.5 text-[10px]'>
                          <span className='rounded bg-muted px-1.5 py-0.5 text-muted-foreground'>
                            {dim.benchmark_basis}
                          </span>
                          <span className='font-mono text-foreground tabular-nums'>
                            +{fmtNumber(dim.total_upside)}
                          </span>
                        </div>
                      </div>
                      <ul className='space-y-1'>
                        {dim.segments.map((seg) => (
                          <li
                            key={seg.segment}
                            className='rounded border bg-card px-2.5 py-1.5 text-xs'
                          >
                            <div className='flex items-center justify-between gap-2'>
                              <span className='font-medium'>{seg.segment}</span>
                              <span className='font-mono text-muted-foreground tabular-nums'>
                                +{fmtNumber(seg.upside)} upside
                              </span>
                            </div>
                            <p className='mt-0.5 font-mono text-[10px] text-muted-foreground tabular-nums'>
                              {fmtNumber(seg.current_value)} → {fmtNumber(seg.benchmark)} (gap{" "}
                              {fmtNumber(seg.gap)})
                            </p>
                          </li>
                        ))}
                      </ul>
                      {dim.other_segments_skipped > 0 && (
                        <p className='text-[10px] text-muted-foreground italic'>
                          + {dim.other_segments_skipped} more segment(s) trimmed
                        </p>
                      )}
                    </div>
                  ))}
                </div>
              )}

              {(result.downstream ?? []).length > 0 && (
                <div className='space-y-1.5'>
                  <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                    Downstream impact
                  </p>
                  <ul className='space-y-1'>
                    {(result.downstream ?? []).map((d) => (
                      <li
                        key={`${d.measure}-${d.path.join("/")}`}
                        className='flex items-center justify-between gap-2 rounded border bg-card px-2.5 py-1.5 text-xs'
                      >
                        <span className='truncate font-mono'>{d.measure}</span>
                        <span className='flex shrink-0 items-center gap-1.5'>
                          <span className='font-mono tabular-nums'>
                            {d.estimated_delta >= 0 ? "+" : ""}
                            {fmtNumber(d.estimated_delta)}
                          </span>
                          <span className='rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground'>
                            {d.confidence}
                          </span>
                        </span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}

              {(result.skipped_dimensions ?? []).length > 0 && (
                <div className='space-y-1.5'>
                  <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                    Skipped dimensions ({(result.skipped_dimensions ?? []).length})
                  </p>
                  <ul className='space-y-1'>
                    {(result.skipped_dimensions ?? []).map((s) => (
                      <li
                        key={s.dimension}
                        className='rounded border border-dashed bg-muted/20 px-2.5 py-1.5 text-[11px]'
                      >
                        <span className='font-mono'>{s.dimension}</span>
                        <span className='ml-1.5 text-muted-foreground'>— {s.reason}</span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── MetricSensitivityView ─────────────────────────────────────────────────────

export const MetricSensitivityView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{ target?: string }>(item.toolInput);
  const output = parseToolJson<SensitivityResult & { error?: string }>(item.toolOutput);
  const error = output?.error;
  const result = output && !error && output.target ? (output as SensitivityResult) : null;

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          <div className='rounded border bg-muted/30 px-2.5 py-2'>
            <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>Target</p>
            <p className='font-medium font-mono text-xs'>
              {input?.target ?? result?.target ?? "—"}
            </p>
          </div>
          {error && <ErrorAlert message={error} />}
          <ToolErrorOrLoading item={item} />
          {result && (
            <div className='space-y-1.5'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                Drivers ({result.drivers.length})
              </p>
              {result.drivers.length === 0 ? (
                <p className='text-muted-foreground text-xs'>
                  No declared drivers found for this measure.
                </p>
              ) : (
                <ul className='space-y-1'>
                  {result.drivers.map((d) => (
                    <li
                      key={`${d.measure}-${d.path.join("/")}`}
                      className='rounded border bg-card px-2.5 py-1.5 text-xs'
                    >
                      <div className='flex items-center justify-between gap-2'>
                        <span className='font-medium font-mono'>{d.measure}</span>
                        <div className='flex shrink-0 items-center gap-1.5 text-[10px]'>
                          <span className='rounded bg-muted px-1.5 py-0.5 text-muted-foreground'>
                            {d.edge_kind}
                          </span>
                          <span
                            className={
                              d.direction === "positive"
                                ? "rounded bg-primary/10 px-1.5 py-0.5 text-primary"
                                : d.direction === "negative"
                                  ? "rounded bg-destructive/10 px-1.5 py-0.5 text-destructive"
                                  : "rounded bg-muted px-1.5 py-0.5 text-muted-foreground"
                            }
                          >
                            {d.direction}
                          </span>
                          <span className='rounded bg-muted px-1.5 py-0.5 text-muted-foreground'>
                            {d.strength}
                          </span>
                        </div>
                      </div>
                      <p className='mt-0.5 truncate font-mono text-[10px] text-muted-foreground'>
                        {d.path.join(" → ")}
                      </p>
                      {((d.effective_coefficient !== undefined &&
                        d.effective_coefficient !== null) ||
                        d.form ||
                        d.lag != null) && (
                        <p className='mt-0.5 text-[10px] text-muted-foreground tabular-nums'>
                          {d.effective_coefficient !== undefined &&
                            d.effective_coefficient !== null &&
                            `coef ${fmtNumber(d.effective_coefficient)}`}
                          {d.form && ` · ${d.form}`}
                          {d.lag != null && ` · lag ${d.lag}d`}
                        </p>
                      )}
                      {d.description && (
                        <p className='mt-0.5 text-[10px] text-muted-foreground italic'>
                          {d.description}
                        </p>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};

// ── PredictImpactView ─────────────────────────────────────────────────────────

export const PredictImpactView = ({ item }: { item: ArtifactItem }) => {
  const input = parseToolJson<{
    changes?: { measure?: string; delta?: number }[];
  }>(item.toolInput);
  const output = parseToolJson<PredictResult & { error?: string }>(item.toolOutput);
  const error = output?.error;
  const result =
    output && !error && Array.isArray(output.impacts) ? (output as PredictResult) : null;

  return (
    <div className='flex h-full flex-col'>
      <div className='flex-1 overflow-auto p-4'>
        <div className='space-y-4'>
          {input?.changes && input.changes.length > 0 && (
            <div className='space-y-1.5'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                Hypothetical changes
              </p>
              <ul className='space-y-1'>
                {input.changes.map((c, i) => (
                  // biome-ignore lint/suspicious/noArrayIndexKey: stable order
                  <li
                    key={i}
                    className='flex items-center justify-between gap-2 rounded border bg-card px-2.5 py-1.5 text-xs'
                  >
                    <span className='truncate font-mono'>{c.measure ?? "?"}</span>
                    <span className='font-mono tabular-nums'>
                      {(c.delta ?? 0) >= 0 ? "+" : ""}
                      {fmtNumber(c.delta ?? 0)}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          {error && <ErrorAlert message={error} />}
          <ToolErrorOrLoading item={item} />
          {result && (
            <div className='space-y-1.5'>
              <p className='text-[10px] text-muted-foreground uppercase tracking-wide'>
                Predicted impacts ({result.impacts.length})
              </p>
              {result.impacts.length === 0 ? (
                <p className='text-muted-foreground text-xs'>
                  No downstream impacts propagated through the metric tree.
                </p>
              ) : (
                <ul className='space-y-1'>
                  {result.impacts.map((d, i) => (
                    // biome-ignore lint/suspicious/noArrayIndexKey: stable order
                    <li key={i} className='rounded border bg-card px-2.5 py-1.5 text-xs'>
                      <div className='flex items-center justify-between gap-2'>
                        <span className='font-medium font-mono'>{d.measure}</span>
                        <span className='flex shrink-0 items-center gap-1.5'>
                          <span className='font-mono tabular-nums'>
                            {d.estimated_delta >= 0 ? "+" : ""}
                            {fmtNumber(d.estimated_delta)}
                          </span>
                          <span className='rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground'>
                            {d.confidence}
                          </span>
                        </span>
                      </div>
                      <p className='mt-0.5 truncate font-mono text-[10px] text-muted-foreground'>
                        {d.path.join(" → ")}
                      </p>
                      {(d.form || d.lag != null) && (
                        <p className='mt-0.5 text-[10px] text-muted-foreground'>
                          {d.form}
                          {d.lag != null && ` · lag ${d.lag}d`}
                        </p>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
        </div>
      </div>
      <TimingBar item={item} />
    </div>
  );
};
