import { Badge } from "@/components/ui/shadcn/badge";
import type { PreviewRunSample } from "@/types/workspace";
import PreviewVerdictBadge from "../PreviewVerdictBadge";

/**
 * An `airway_sample` run's detail: the window/resources requested, then —
 * once the sample ran — its schema compare against live. Counts and
 * verdicts only, same "never a row value" rule as `PreviewCompareView`.
 * Findings reuse the checks' shape and badge (`PreviewVerdictBadge`).
 */
export default function PreviewSampleView({ sample }: { sample: PreviewRunSample }) {
  return (
    <div className='flex flex-col gap-2' data-testid='preview-sample'>
      <div className='flex flex-wrap items-center gap-2 text-xs'>
        <span className='font-mono'>{sample.pipeline}</span>
        <span className='text-muted-foreground'>dataset {sample.dataset}</span>
        {sample.window ? (
          <span className='text-muted-foreground'>
            {sample.window.from} → {sample.window.to}
          </span>
        ) : (
          <span className='text-muted-foreground'>no window</span>
        )}
        {sample.resources.length > 0 && (
          <span className='text-muted-foreground'>resources: {sample.resources.join(", ")}</span>
        )}
        {sample.wall_clock_capped && <Badge variant='outline'>Time-capped</Badge>}
        {sample.partial && <Badge variant='outline'>Partial</Badge>}
      </div>
      {sample.partial_reason && (
        <p className='text-muted-foreground text-xs'>{sample.partial_reason}</p>
      )}
      {sample.record_error && (
        <p className='text-destructive text-xs' data-testid='preview-sample-record-error'>
          {sample.record_error}
        </p>
      )}
      {sample.preview_pipeline && (
        <p className='text-xs'>
          Sampled into <span className='font-mono'>{sample.preview_pipeline}</span>
          {sample.tables && sample.tables.length > 0 && (
            <>
              {" "}
              — tables: <span className='font-mono'>{sample.tables.join(", ")}</span>
            </>
          )}
        </p>
      )}
      {sample.compared_with_live && sample.verdict && (
        <div className='flex items-center gap-2'>
          <span className='text-muted-foreground text-xs'>Compared with live:</span>
          <PreviewVerdictBadge verdict={sample.verdict} />
        </div>
      )}
      {sample.findings.length > 0 && (
        <ul className='flex flex-col gap-2'>
          {sample.findings.map((finding, i) => (
            // Findings carry no id of their own — same key convention as the checks panel.
            // biome-ignore lint/suspicious/noArrayIndexKey: findings have no stable id
            <li key={`${finding.kind}-${i}`} className='flex flex-col gap-1 border-t pt-2 text-sm'>
              <div className='flex flex-wrap items-center gap-2'>
                <PreviewVerdictBadge verdict={finding.verdict} />
                <span className='text-muted-foreground text-xs'>{finding.kind}</span>
              </div>
              <p>{finding.detail}</p>
              {finding.prod_action && (
                <p className='text-muted-foreground text-xs'>Prod action: {finding.prod_action}</p>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
