import { DollarSign } from "lucide-react";
import { resolveColor } from "@/components/Echarts/resolveColor";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle
} from "@/components/ui/shadcn/card";
import { useExecutionCost } from "@/hooks/api/useExecutionAnalytics";

const DOT_TOKENS = ["--span-llm", "--span-agent", "--span-tool", "--span-sql", "--span-retrieval"];

const fmtMs = (ms: number) => (ms >= 1000 ? `${(ms / 1000).toFixed(1)}s` : `${Math.round(ms)}ms`);

function fmtTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return `${n}`;
}

interface CostByModelProps {
  projectId: string;
  days: number;
}

export default function CostByModel({ projectId, days }: CostByModelProps) {
  const { data, isLoading } = useExecutionCost(projectId, days);
  const rows = data?.byModel ?? [];
  // A model with no entry in the price map contributes $0 to the total, which
  // would silently understate spend — flag the headline as a partial estimate.
  const hasUnpriced = rows.some((m) => m.costUsd === 0 && m.tokens > 0);

  return (
    <Card className='bg-transparent shadow-none'>
      <CardHeader className='pb-2'>
        <div className='flex items-center justify-between'>
          <div className='flex items-center gap-2'>
            <DollarSign className='h-5 w-5 text-primary' />
            <CardTitle>Cost &amp; Tokens by Model</CardTitle>
          </div>
          {data ? (
            <span
              className='font-mono text-muted-foreground text-xs tabular-nums'
              title={
                hasUnpriced
                  ? "Some models have no price in the price map — total is a partial estimate"
                  : undefined
              }
            >
              {fmtTokens(data.totalTokens)} tok · {hasUnpriced ? "~" : ""}$
              {data.totalCostUsd.toFixed(2)}
              {hasUnpriced ? " (partial)" : ""}
            </span>
          ) : null}
        </div>
        <CardDescription>LLM usage over the selected range</CardDescription>
      </CardHeader>
      <CardContent>
        {rows.length === 0 ? (
          <div className='flex h-24 items-center justify-center text-muted-foreground text-sm'>
            {isLoading ? "Loading…" : "No LLM calls in range"}
          </div>
        ) : (
          <div className='overflow-x-auto'>
            <table className='w-full text-sm'>
              <thead>
                <tr className='border-border border-b text-muted-foreground text-xs uppercase tracking-wide'>
                  <th className='py-2 text-left font-medium'>Model</th>
                  <th className='py-2 text-right font-medium'>Calls</th>
                  <th className='py-2 text-right font-medium'>Tokens</th>
                  <th className='py-2 text-right font-medium'>Cost</th>
                  <th className='py-2 text-right font-medium'>p95</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((m, i) => {
                  const unknownCost = m.costUsd === 0 && m.tokens > 0;
                  return (
                    <tr key={m.model} className='border-border/60 border-b last:border-0'>
                      <td className='py-2'>
                        <span className='flex items-center gap-2 font-mono text-xs'>
                          <span
                            className='inline-block h-2 w-2 '
                            style={{ background: resolveColor(DOT_TOKENS[i % DOT_TOKENS.length]) }}
                          />
                          {m.model}
                        </span>
                      </td>
                      <td className='py-2 text-right font-mono tabular-nums'>
                        {m.calls.toLocaleString()}
                      </td>
                      <td className='py-2 text-right font-mono tabular-nums'>
                        {fmtTokens(m.tokens)}
                      </td>
                      <td className='py-2 text-right font-mono tabular-nums'>
                        {unknownCost ? (
                          <span className='text-muted-foreground' title='No price for this model'>
                            —
                          </span>
                        ) : (
                          `$${m.costUsd.toFixed(2)}`
                        )}
                      </td>
                      <td className='py-2 text-right font-mono tabular-nums'>{fmtMs(m.p95Ms)}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </CardContent>
    </Card>
  );
}
