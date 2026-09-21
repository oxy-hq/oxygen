import { Check, Copy } from "lucide-react";
import { useState } from "react";
import { cn } from "@/libs/utils/cn";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { QueueRow } from "@/services/api/internalJobs";
import { relativeTime } from "../../../utils";
import { summarizeSpec } from "./specSummary";

/**
 * The expanded debug surface for a single failed/dead job — the answer to
 * "why did this break and whose was it?". Three blocks:
 *   1. Error — the run's `error_message`, full width, monospace.
 *   2. Context — tenant (workspace / org / user) + run identity, with
 *      copyable ids.
 *   3. Spec — the decoded TaskSpec (agent / question / automation / …).
 *
 * Every value is selectable; ids carry a one-click copy affordance because
 * the first thing an operator does is paste a run_id into a log query.
 */
export const JobDebugPanel = ({ row }: { row: QueueRow }) => {
  const { type, fields } = summarizeSpec(row.spec);

  return (
    <div className='space-y-4 border-border/60 border-t bg-muted/20 px-4 py-4'>
      {row.run_error_message ? (
        <DebugBlock label='Error'>
          <pre className='max-h-48 overflow-auto whitespace-pre-wrap break-words rounded-md border border-destructive/30 bg-destructive/5 p-2.5 font-mono text-[11px] text-destructive leading-relaxed'>
            {row.run_error_message}
          </pre>
        </DebugBlock>
      ) : (
        <DebugBlock label='Error'>
          <p className='text-muted-foreground text-xs italic'>
            No error message recorded on the run.
          </p>
        </DebugBlock>
      )}

      <div className='grid gap-4 lg:grid-cols-2'>
        <DebugBlock label='Context'>
          <dl className='grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-xs'>
            <ContextRow label='Workspace' value={row.workspace_name} mono={row.workspace_id} />
            <ContextRow label='Org' value={row.org_name} mono={row.org_id} />
            <ContextRow label='User' value={row.originating_user_email} />
            <ContextRow label='Run' value={row.run_status ?? "—"} mono={row.run_id} />
          </dl>
        </DebugBlock>

        <DebugBlock label='Claim history'>
          <dl className='grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-xs'>
            <PlainRow label='Claims' value={`${row.claim_count} / ${row.max_claims}`} />
            <PlainRow label='Worker' value={row.worker_id ?? "—"} mono />
            <PlainRow label='Claimed' value={relativeTime(row.claimed_at)} />
            <PlainRow label='Heartbeat' value={relativeTime(row.last_heartbeat)} />
            <PlainRow label='Created' value={relativeTime(row.created_at)} />
            <PlainRow label='Updated' value={relativeTime(row.updated_at)} />
          </dl>
        </DebugBlock>
      </div>

      <DebugBlock label={type ? `Spec · ${type}` : "Spec"}>
        {fields.length > 0 ? (
          <dl className='grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-xs'>
            {fields.map((f) => (
              <PlainRow key={f.label} label={f.label} value={f.value} mono />
            ))}
          </dl>
        ) : (
          <p className='text-muted-foreground text-xs italic'>No decodable spec.</p>
        )}
      </DebugBlock>
    </div>
  );
};

const DebugBlock = ({ label, children }: { label: string; children: React.ReactNode }) => (
  <div className='space-y-1.5'>
    <span className='font-medium text-[10px] text-muted-foreground uppercase tracking-[0.14em]'>
      {label}
    </span>
    {children}
  </div>
);

/** A context row whose primary value can be null (renders "—") plus an
 * optional copyable id underneath. */
const ContextRow = ({
  label,
  value,
  mono
}: {
  label: string;
  value: string | null;
  mono?: string | null;
}) => (
  <>
    <dt className='text-muted-foreground'>{label}</dt>
    <dd className='min-w-0'>
      <span className={cn(!value && "text-muted-foreground")}>{value ?? "—"}</span>
      {mono ? (
        <span className='mt-0.5 flex items-center gap-1'>
          <span className='truncate font-mono text-[10px] text-muted-foreground'>{mono}</span>
          <CopyChip value={mono} />
        </span>
      ) : null}
    </dd>
  </>
);

const PlainRow = ({ label, value, mono }: { label: string; value: string; mono?: boolean }) => (
  <>
    <dt className='text-muted-foreground'>{label}</dt>
    <dd className={cn("min-w-0 break-words", mono && "font-mono text-[11px]")}>{value}</dd>
  </>
);

const CopyChip = ({ value }: { value: string }) => {
  const [copied, setCopied] = useState(false);
  return (
    <button
      type='button'
      onClick={() => {
        navigator.clipboard?.writeText(value).then(
          () => {
            setCopied(true);
            window.setTimeout(() => setCopied(false), 1200);
          },
          () => undefined
        );
      }}
      className='shrink-0 text-muted-foreground transition-colors hover:text-foreground'
      aria-label={`Copy ${value}`}
    >
      {copied ? (
        <Check className={cn("size-3", ADMIN_TONE.ok.text)} />
      ) : (
        <Copy className='size-3' />
      )}
    </button>
  );
};
