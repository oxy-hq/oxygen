import { ShieldAlert } from "lucide-react";
import { type FormEvent, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { useStartAssume } from "@/hooks/api/adminAssume";
import type { AssumeSession } from "@/types/adminAssume";
import type { FactoryTarget } from "./WorkspacePicker";

/**
 * The reason, and the session that opens the Factory. The same consent moment as
 * `AssumeRoleDialog` (an audited, 60-minute, Owner-level session under your own
 * identity), with the landing set to the chosen workspace's Factory.
 */
export function StartSessionStep({
  target,
  actingAs,
  onBack,
  onBeforeStart
}: {
  target: FactoryTarget;
  /** A live session for ANOTHER org: one at a time, so this one cannot start. */
  actingAs?: AssumeSession;
  onBack?: () => void;
  /** Records where the session lands and returns to, before it starts. */
  onBeforeStart: (target: FactoryTarget) => void;
}) {
  const [reason, setReason] = useState("");
  const start = useStartAssume();

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!reason.trim() || actingAs) return;
    onBeforeStart(target);
    // On success the page reloads into the Factory (`useStartAssume`).
    start.mutate({ orgId: target.orgId, reason: reason.trim() });
  };

  return (
    <form onSubmit={submit} className='space-y-4 px-6 pt-2 pb-6' data-testid='open-factory-start'>
      {actingAs ? (
        <p className='text-muted-foreground text-sm'>
          You're acting as {actingAs.org_name ?? actingAs.org_slug ?? "another organization"}. Stop
          that session first, then open this one.
        </p>
      ) : (
        <>
          <p className='flex gap-2 text-muted-foreground text-sm'>
            <ShieldAlert className='mt-0.5 size-4 shrink-0 text-warning' aria-hidden />
            <span>
              Opening it starts a 60-minute session as an owner of {target.orgName}. It can't be
              extended, and its start and end are written to the audit log under your identity.
            </span>
          </p>
          <div className='space-y-1.5'>
            <Label htmlFor='open-factory-reason'>Reason</Label>
            <Input
              id='open-factory-reason'
              autoFocus
              placeholder='e.g. publishing their new forecast app'
              value={reason}
              onChange={(e) => setReason(e.target.value)}
              data-testid='open-factory-reason'
            />
          </div>
        </>
      )}
      <div className='flex justify-end gap-2'>
        {onBack && (
          <Button type='button' variant='ghost' onClick={onBack}>
            Back
          </Button>
        )}
        <Button
          type='submit'
          disabled={!!actingAs || !reason.trim() || start.isPending}
          data-testid='open-factory-submit'
        >
          {start.isPending ? "Starting…" : "Start session and open"}
        </Button>
      </div>
    </form>
  );
}
