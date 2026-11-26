import { AlertTriangle, Check, Loader2, Pencil, X } from "lucide-react";
import type React from "react";
import { useState } from "react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import useBudgetStatus from "@/hooks/api/cameras/useBudgetStatus";
import useSetBudget from "@/hooks/api/cameras/useSetBudget";
import { type BudgetStatus, formatCostUsd } from "@/services/api/cameras";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * Monthly VLM budget surface. Renders three states:
 *
 *  1. No budget set — small "Set monthly budget" prompt with an
 *     inline editor. No alert, since we have nothing to compare
 *     against.
 *  2. Budget set, projection comfortably below it — green
 *     "$X spent · $Y projected · $Z budget" summary.
 *  3. Projection or actual spend within reach of (or over) the
 *     budget — amber/red banner with the same numbers + an
 *     explicit threshold hit.
 *
 * The same card hosts the editor on click — keeps the surface to
 * one place rather than scattering banner + settings field.
 */
const AMBER_THRESHOLD = 0.8;
const RED_THRESHOLD = 1.0;

type Severity = "ok" | "amber" | "red";

function severityForStatus(status: BudgetStatus): Severity {
  const worst = Math.max(status.percent_projected ?? 0, status.percent_consumed ?? 0);
  if (worst >= RED_THRESHOLD) return "red";
  if (worst >= AMBER_THRESHOLD) return "amber";
  return "ok";
}

const SEVERITY_STYLES: Record<Severity, { box: string; text: string; badge: string }> = {
  ok: {
    box: "border-border bg-muted/40",
    text: "text-foreground",
    badge: "bg-primary/10 text-primary"
  },
  amber: {
    box: "border-warning/40 bg-warning/5",
    text: "text-warning",
    badge: "bg-warning/15 text-warning"
  },
  red: {
    box: "border-destructive bg-destructive/5",
    text: "text-destructive",
    badge: "bg-destructive/15 text-destructive"
  }
};

const VlmBudgetCard: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  const { data: status, isLoading } = useBudgetStatus(workspace?.id);
  const setBudget = useSetBudget(workspace?.id);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState("");

  const openEditor = () => {
    const current = status?.budget_micros;
    setDraft(current != null ? (current / 1_000_000).toFixed(2) : "");
    setEditing(true);
  };

  const cancelEdit = () => {
    setEditing(false);
    setDraft("");
  };

  const handleSave = async () => {
    const trimmed = draft.trim();
    let micros: number | null = null;
    if (trimmed !== "") {
      const dollars = Number(trimmed);
      if (!Number.isFinite(dollars) || dollars < 0) {
        toast.error("Enter a non-negative number, or leave blank to clear.");
        return;
      }
      micros = Math.round(dollars * 1_000_000);
    }
    try {
      await setBudget.mutateAsync(micros);
      toast.success(micros == null ? "Budget cleared." : "Budget updated.");
      setEditing(false);
    } catch (e) {
      toast.error(e instanceof Error ? e.message : "Failed to save budget.");
    }
  };

  if (isLoading || !status) {
    return (
      <div className='flex items-center gap-2 border border-border bg-muted/40 p-3 text-muted-foreground text-sm'>
        <Loader2 className='h-4 w-4 animate-spin' />
        Loading budget…
      </div>
    );
  }

  const severity = severityForStatus(status);
  const styles = SEVERITY_STYLES[severity];

  return (
    <div className={`flex flex-col gap-3 border p-3 ${styles.box}`}>
      <div className='flex items-start justify-between gap-3'>
        <div className='flex items-start gap-2'>
          {severity === "ok" ? null : (
            <AlertTriangle className={`mt-0.5 h-4 w-4 ${styles.text}`} aria-hidden />
          )}
          <div className='flex flex-col'>
            <span className={`font-medium text-sm ${styles.text}`}>
              {status.budget_micros == null
                ? "No monthly VLM budget set"
                : severity === "red"
                  ? "Projected to exceed monthly budget"
                  : severity === "amber"
                    ? "Approaching monthly budget"
                    : "Monthly VLM spend"}
            </span>
            <span className='text-muted-foreground text-xs'>{summaryLine(status)}</span>
          </div>
        </div>
        {!editing && (
          <Button
            size='sm'
            variant='outline'
            onClick={openEditor}
            aria-label={status.budget_micros == null ? "Set budget" : "Edit budget"}
          >
            <Pencil className='mr-1.5 h-3.5 w-3.5' />
            {status.budget_micros == null ? "Set budget" : "Edit"}
          </Button>
        )}
      </div>

      {editing && (
        <div className='flex flex-col gap-2 border border-border bg-background p-2'>
          <Label htmlFor='vlm-budget' className='text-muted-foreground text-xs uppercase'>
            Monthly budget (USD)
          </Label>
          <div className='flex items-center gap-2'>
            <Input
              id='vlm-budget'
              type='number'
              min='0'
              step='1'
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              placeholder='e.g. 250'
              className='h-8 max-w-32 text-xs'
              autoFocus
              disabled={setBudget.isPending}
            />
            <span className='text-muted-foreground text-xs'>Leave blank to clear the limit.</span>
          </div>
          <div className='flex items-center gap-2'>
            <Button
              size='sm'
              onClick={handleSave}
              disabled={setBudget.isPending}
              aria-label='Save budget'
            >
              {setBudget.isPending ? (
                <Loader2 className='mr-1.5 h-3.5 w-3.5 animate-spin' />
              ) : (
                <Check className='mr-1.5 h-3.5 w-3.5' />
              )}
              Save
            </Button>
            <Button
              size='sm'
              variant='ghost'
              onClick={cancelEdit}
              disabled={setBudget.isPending}
              aria-label='Cancel'
            >
              <X className='mr-1.5 h-3.5 w-3.5' />
              Cancel
            </Button>
          </div>
        </div>
      )}

      {status.budget_micros != null && (
        <div className='flex flex-wrap items-center gap-2 font-mono text-xs'>
          <Badge variant='outline' className={styles.badge}>
            MTD {formatCostUsd(status.month_to_date_micros)}
          </Badge>
          <Badge variant='outline' className={styles.badge}>
            Projected {formatCostUsd(status.projected_month_micros)}
          </Badge>
          <Badge variant='secondary'>Budget {formatCostUsd(status.budget_micros)}</Badge>
          {status.percent_projected != null && (
            <span className={`text-xs ${styles.text}`}>
              {Math.min(Math.round(status.percent_projected * 100), 999)}% of budget projected
            </span>
          )}
        </div>
      )}
    </div>
  );
};

function summaryLine(status: BudgetStatus): string {
  const day = `Day ${status.day_of_month} of ${status.days_in_month}`;
  if (status.budget_micros == null) {
    return `${day} · ${formatCostUsd(status.month_to_date_micros)} spent so far this month`;
  }
  return `${day} · spend projected to ${formatCostUsd(status.projected_month_micros)} of ${formatCostUsd(
    status.budget_micros
  )}`;
}

export default VlmBudgetCard;
