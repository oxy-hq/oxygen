import { CheckCircle2, PlugZap, XCircle } from "lucide-react";
import { Button } from "@/components/ui/shadcn/button";
import type { useTestOltpConnection } from "@/hooks/api/oltp/useAdminOltp";
import { cn } from "@/libs/shadcn/utils";
import { ADMIN_TONE } from "@/pages/admin/components/adminTone";
import type { OltpConnectionTest as Result } from "@/services/api/oltp";

/** What each stage a probe can stop at means, said once beside the message. */
const STAGE: Record<NonNullable<Result["stage"]>, string> = {
  resolve: "Oxy could not build a connection",
  connect: "The database refused the session",
  query: "Connected, but SELECT 1 failed",
  timeout: "No answer"
};

/**
 * Does the database answer — asked of the database itself.
 *
 * The status above is Oxy's own record of the tenant and never touches the
 * database, so it read `active` all through the 2026-10-21 outage while Neon
 * refused every session for its quota. This opens one session as the analyst,
 * on a click only (it wakes a suspended compute), and prints what came back in
 * full: the server's own words are the diagnosis.
 */
export const OltpConnectionTest = ({
  probe
}: {
  probe: ReturnType<typeof useTestOltpConnection>;
}) => (
  <div className='flex flex-col gap-1.5' data-testid='admin-org-oltp-test'>
    <div className='flex flex-wrap items-center gap-2'>
      <Button
        size='sm'
        variant='outline'
        className='h-7 w-fit px-2'
        disabled={probe.isPending}
        onClick={() => probe.mutate()}
        data-testid='admin-org-oltp-test-run'
      >
        <PlugZap className='size-3' />
        {probe.isPending ? "Testing…" : "Test connection"}
      </Button>
      {probe.data && <Verdict result={probe.data} />}
    </div>
    {probe.data && !probe.data.ok && probe.data.error && (
      <pre
        className='max-h-60 overflow-auto whitespace-pre-wrap break-words rounded bg-muted/40 px-3 py-2 font-mono text-xs leading-relaxed'
        data-testid='admin-org-oltp-test-error'
      >
        {probe.data.error}
      </pre>
    )}
  </div>
);

const Verdict = ({ result }: { result: Result }) => {
  const tone = ADMIN_TONE[result.ok ? "ok" : "danger"];
  const Icon = result.ok ? CheckCircle2 : XCircle;
  return (
    <span
      className={cn("inline-flex items-center gap-1 text-xs", tone.text)}
      data-testid='admin-org-oltp-test-verdict'
    >
      <Icon className='size-3 shrink-0' />
      {result.ok ? (
        <span>
          Answered as <span className='font-mono'>{result.role}</span> in{" "}
          <span className='tabular-nums'>{result.elapsed_ms} ms</span>
        </span>
      ) : (
        <span>
          {result.stage ? STAGE[result.stage] : "Failed"} after{" "}
          <span className='tabular-nums'>{result.elapsed_ms} ms</span>
        </span>
      )}
    </span>
  );
};
