import { Loader2 } from "lucide-react";
import type React from "react";
import { useMemo, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import { Label } from "@/components/ui/shadcn/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import useBudgetStatus from "@/hooks/api/cameras/useBudgetStatus";
import useEdgeBoxCost from "@/hooks/api/cameras/useEdgeBoxCost";
import useWorkspaceCost from "@/hooks/api/cameras/useWorkspaceCost";
import { type CostRange, formatCostUsd } from "@/services/api/cameras";

/**
 * Per-box VLM cost surface. Combines:
 *  - This box's spend over a selectable window (`useEdgeBoxCost`)
 *  - A per-model breakdown of where that spend went
 *  - Workspace context: this box's rank + share of the workspace
 *    total (`useWorkspaceCost`)
 *  - Workspace-level budget threshold (`useBudgetStatus`) so the
 *    operator can see how this box contributes to the month-end
 *    projection without bouncing back to the budget card.
 *
 * Each fetch is independently cached + slow-moving (5min stale +
 * refetch), so opening the tab against a 501-state Airhouse just
 * renders an empty-data card per query rather than blocking the
 * whole page.
 */
type Props = {
  workspaceId: string | undefined;
  boxId: string | undefined;
};

const RANGES: { value: CostRange; label: string }[] = [
  { value: "24h", label: "Last 24 hours" },
  { value: "7d", label: "Last 7 days" },
  { value: "30d", label: "Last 30 days" }
];

const BudgetPanel: React.FC<Props> = ({ workspaceId, boxId }) => {
  const [range, setRange] = useState<CostRange>("24h");

  const {
    data: boxCost,
    isLoading: boxLoading,
    error: boxError
  } = useEdgeBoxCost(workspaceId, boxId, range);
  const { data: workspaceCost } = useWorkspaceCost(workspaceId, range);
  const { data: budget } = useBudgetStatus(workspaceId);

  const ranking = useMemo(() => {
    if (!boxId || !workspaceCost || workspaceCost.by_box.length === 0) return null;
    const rank = workspaceCost.by_box.findIndex((b) => b.edge_box_id === boxId);
    if (rank < 0) return null;
    const share =
      workspaceCost.cost_usd_micros > 0
        ? (workspaceCost.by_box[rank].cost_usd_micros / workspaceCost.cost_usd_micros) * 100
        : 0;
    return {
      rank: rank + 1,
      total: workspaceCost.by_box.length,
      sharePct: share
    };
  }, [boxId, workspaceCost]);

  return (
    <div className='flex flex-col gap-3'>
      <div className='flex items-end gap-3'>
        <div className='flex flex-col gap-1'>
          <Label htmlFor='budget-range' className='text-muted-foreground text-xs uppercase'>
            Window
          </Label>
          <Select value={range} onValueChange={(v) => setRange(v as CostRange)}>
            <SelectTrigger id='budget-range' className='h-8 w-44 text-xs'>
              <SelectValue />
            </SelectTrigger>
            <SelectContent className={DESIGN_REFERENCE}>
              {RANGES.map((r) => (
                <SelectItem key={r.value} value={r.value}>
                  {r.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        {boxLoading && (
          <div className='flex items-center gap-1.5 text-muted-foreground text-xs'>
            <Loader2 className='h-3 w-3 animate-spin' />
            Loading…
          </div>
        )}
      </div>

      {boxError ? (
        <ErrorBanner message={boxError.message} />
      ) : (
        <div className='grid grid-cols-1 gap-3 md:grid-cols-3'>
          <HeadlineCard
            label='Spend'
            value={boxCost ? formatCostUsd(boxCost.cost_usd_micros) : "—"}
            sub={
              boxCost
                ? `${boxCost.reports} ${boxCost.reports === 1 ? "report" : "reports"} · ${boxCost.tokens_used.toLocaleString()} tokens`
                : "no reports in this window"
            }
          />
          <HeadlineCard
            label='Rank in workspace'
            value={ranking ? `#${ranking.rank} of ${ranking.total}` : "—"}
            sub={
              ranking
                ? `${ranking.sharePct.toFixed(1)}% of workspace spend`
                : workspaceCost
                  ? "no spend recorded"
                  : "loading…"
            }
          />
          <HeadlineCard
            label='Workspace budget (MTD)'
            value={budget ? formatCostUsd(budget.month_to_date_micros) : "—"}
            sub={
              budget
                ? budget.budget_micros == null
                  ? "no budget configured"
                  : `${((budget.percent_consumed ?? 0) * 100).toFixed(0)}% of ${formatCostUsd(budget.budget_micros)} ceiling`
                : "loading…"
            }
            tone={
              budget?.percent_projected != null && budget.percent_projected >= 1
                ? "danger"
                : "default"
            }
          />
        </div>
      )}

      <ModelBreakdown rows={boxCost?.by_model ?? []} loading={boxLoading} hasError={!!boxError} />
    </div>
  );
};

const HeadlineCard: React.FC<{
  label: string;
  value: string;
  sub: string;
  tone?: "default" | "danger";
}> = ({ label, value, sub, tone = "default" }) => {
  const toneClass =
    tone === "danger" ? "border-destructive/40 bg-destructive/5" : "border-border bg-muted/20";
  return (
    <div className={`flex flex-col gap-1 border p-3 ${toneClass}`}>
      <div className='text-[10px] text-muted-foreground uppercase tracking-wide'>{label}</div>
      <div className='font-semibold text-base'>{value}</div>
      <div className='text-muted-foreground text-xs'>{sub}</div>
    </div>
  );
};

const ModelBreakdown: React.FC<{
  rows: Array<{
    model: string;
    reports: number;
    tokens_used: number;
    cost_usd_micros: number | null;
  }>;
  loading: boolean;
  hasError: boolean;
}> = ({ rows, loading, hasError }) => {
  if (hasError) return null;
  return (
    <div className='flex flex-col gap-2 border border-border bg-card p-3'>
      <div className='flex items-center justify-between'>
        <div className='text-[10px] text-muted-foreground uppercase tracking-wide'>By model</div>
        {loading && <Loader2 className='h-3 w-3 animate-spin text-muted-foreground' />}
      </div>
      {rows.length === 0 ? (
        <div className='text-muted-foreground text-xs'>
          No VLM reports for this box in the selected window.
        </div>
      ) : (
        <table className='w-full text-xs'>
          <thead className='text-muted-foreground'>
            <tr>
              <th className='py-1 text-left font-normal'>Model</th>
              <th className='py-1 text-right font-normal'>Reports</th>
              <th className='py-1 text-right font-normal'>Tokens</th>
              <th className='py-1 text-right font-normal'>Cost</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={row.model} className='border-border border-t'>
                <td className='py-1.5'>
                  <code className='text-xs'>{row.model}</code>
                </td>
                <td className='py-1.5 text-right'>{row.reports}</td>
                <td className='py-1.5 text-right'>{row.tokens_used.toLocaleString()}</td>
                <td className='py-1.5 text-right'>
                  {row.cost_usd_micros == null ? (
                    <Badge variant='outline' className='text-[10px]'>
                      unpriced
                    </Badge>
                  ) : (
                    formatCostUsd(row.cost_usd_micros)
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
};

const ErrorBanner: React.FC<{ message: string }> = ({ message }) => (
  <div className='border border-destructive bg-destructive/10 p-3 text-destructive text-sm'>
    Failed to load cost: {message}
  </div>
);

export default BudgetPanel;
