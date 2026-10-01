import { ChevronDown, ChevronRight } from "lucide-react";
import { useState } from "react";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import type { PreviewRunStep, PreviewRunStepStatus } from "@/types/workspace";

const STATUS_VARIANT: Record<
  PreviewRunStepStatus,
  "default" | "secondary" | "destructive" | "outline"
> = {
  succeeded: "default",
  failed: "destructive",
  held: "secondary",
  running: "secondary",
  pending: "outline"
};

const STATUS_LABEL: Record<PreviewRunStepStatus, string> = {
  succeeded: "Succeeded",
  failed: "Failed",
  held: "Held",
  running: "Running",
  pending: "Pending"
};

/**
 * One step of a run. A `held` step succeeded so the procedure could continue
 * — the interesting fact is what it WOULD have written, so that's what this
 * expands to: verb, targets, reason, and (for SQL steps) the rendered
 * statement, collapsed by default since it can be long.
 */
export default function PreviewRunStepRow({ step }: { step: PreviewRunStep }) {
  const [sqlOpen, setSqlOpen] = useState(false);
  const testId = `preview-run-step-${step.name}`;

  return (
    <div className='flex flex-col gap-1.5 rounded-md border p-2.5' data-testid={testId}>
      <div className='flex flex-wrap items-center gap-2'>
        <span className='font-mono text-sm'>{step.name}</span>
        <span className='text-muted-foreground text-xs'>{step.kind}</span>
        <Badge variant={STATUS_VARIANT[step.status]} className='ml-auto'>
          {STATUS_LABEL[step.status]}
        </Badge>
      </div>
      {step.held && (
        <div className='flex flex-col gap-1 border-t pt-1.5 text-xs'>
          <p>
            <span className='font-medium'>{step.held.verb}</span>{" "}
            <span className='text-muted-foreground'>would have written to</span>{" "}
            <span className='font-mono'>{step.held.targets.join(", ")}</span>
          </p>
          <p className='text-muted-foreground'>{step.held.reason}</p>
          {step.held.sql && (
            <div>
              <Button
                variant='ghost'
                size='sm'
                className='h-6 px-1.5 text-xs'
                aria-expanded={sqlOpen}
                onClick={() => setSqlOpen((v) => !v)}
                data-testid={`${testId}-toggle-sql`}
              >
                {sqlOpen ? <ChevronDown className='size-3' /> : <ChevronRight className='size-3' />}
                {sqlOpen ? "Hide SQL" : "Show SQL"}
              </Button>
              {sqlOpen && (
                <pre className='mt-1 max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-md bg-muted p-2 font-mono text-xs'>
                  {step.held.sql}
                </pre>
              )}
            </div>
          )}
        </div>
      )}
      {step.redirected && (
        <PreviewRunRedirectedNotes redirected={step.redirected} testId={testId} />
      )}
    </div>
  );
}

/**
 * What went to the preview's own copies instead of being held — a redirected
 * step succeeded, so this is informational, not a refusal like `held` above.
 */
function PreviewRunRedirectedNotes({
  redirected,
  testId
}: {
  redirected: NonNullable<PreviewRunStep["redirected"]>;
  testId: string;
}) {
  return (
    <div
      className='flex flex-col gap-1 border-t pt-1.5 text-xs'
      data-testid={`${testId}-redirected`}
    >
      {redirected.writes.length > 0 && (
        <p>
          <span className='font-medium'>Wrote to the preview's copy of</span>{" "}
          <span className='font-mono'>
            {redirected.writes.map((w) => `${w.live} → ${w.preview}`).join(", ")}
          </span>
        </p>
      )}
      {redirected.reads.length > 0 && (
        <p>
          <span className='font-medium'>Read the preview's copy of</span>{" "}
          <span className='font-mono'>
            {redirected.reads.map((r) => `${r.live} → ${r.preview}`).join(", ")}
          </span>
        </p>
      )}
      {redirected.copies.length > 0 && (
        <p className='text-muted-foreground'>
          {redirected.copies
            .map((c) => `${c.live} (${c.state === "partial" ? "partial copy" : "shadow copy"})`)
            .join(", ")}
        </p>
      )}
    </div>
  );
}
