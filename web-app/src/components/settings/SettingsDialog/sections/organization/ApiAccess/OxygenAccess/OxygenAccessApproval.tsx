import { ShieldCheck } from "lucide-react";
import { toast } from "sonner";
import { Badge } from "@/components/ui/shadcn/badge";
import { Button } from "@/components/ui/shadcn/button";
import {
  useApproveOxygenAccess,
  useOxygenAccess,
  useRevokeOxygenAccess
} from "@/hooks/api/oxygenAccess";
import type { OxygenAction, OxygenActionStatus } from "@/types/oxygenAccess";
import { ListState } from "../shared/ListState";
import { describeApiError } from "../utils/errors";

const STATUS: Record<
  OxygenActionStatus,
  { label: string; variant: "default" | "secondary" | "outline" }
> = {
  pending: { label: "Waiting for approval", variant: "outline" },
  approved: { label: "Approved", variant: "default" },
  approved_unused: { label: "Approved, no longer asked for", variant: "secondary" }
};

const when = (at: string | null) => (at ? new Date(at).toLocaleString() : null);

function ActionRow({ action }: { action: OxygenAction }) {
  const status = STATUS[action.status];
  return (
    <li className='flex flex-col gap-1 rounded-md border p-3' data-testid='oxygen-access-action'>
      <div className='flex items-start justify-between gap-3'>
        <p className='text-sm'>{action.description}</p>
        <Badge variant={status.variant} className='shrink-0'>
          {status.label}
        </Badge>
      </div>
      {action.targets.length > 0 && (
        <ul className='flex flex-col gap-1' data-testid='oxygen-access-targets'>
          {action.targets.map((t) => (
            <li key={t.name} className='flex items-center justify-between gap-3 text-sm'>
              <span>
                Pipeline <code className='font-mono text-xs'>{t.name}</code>
              </span>
              <Badge variant={STATUS[t.status].variant} className='shrink-0'>
                {STATUS[t.status].label}
              </Badge>
            </li>
          ))}
        </ul>
      )}
      <p className='text-xs text-muted-foreground'>
        {action.functions.length > 0
          ? `Asked for by ${action.functions.map((f) => `\`${f}\``).join(", ")}.`
          : "No function of the live build asks for it."}
        {action.approved_at &&
          ` Approved ${when(action.approved_at)}${action.approved_by ? ` by ${action.approved_by}` : ""}.`}
      </p>
    </li>
  );
}

interface OxygenAccessApprovalProps {
  orgId: string;
  appId: string;
}

/**
 * One app's request to act in Oxygen itself, and the org admin's decision on
 * it. The approval page and the list's dialog both render this.
 *
 * What it must make plain, because it is what the admin is trusting: who put
 * the live build there, each action in the platform's words, and that Oxygen
 * will check the app rather than the person using it.
 */
export function OxygenAccessApproval({ orgId, appId }: OxygenAccessApprovalProps) {
  const { data, isPending, error, refetch } = useOxygenAccess(orgId, appId);
  const approve = useApproveOxygenAccess();
  const revoke = useRevokeOxygenAccess();

  const pending = data?.actions.filter((a) => a.status === "pending") ?? [];

  const onApprove = () =>
    approve.mutate(
      { orgId, appId, actions: pending.map((a) => a.action) },
      {
        onSuccess: (saved) => toast.success(`${saved.app_name} is approved.`),
        onError: (err) => toast.error(describeApiError(err, "Could not approve the app."))
      }
    );
  const onRevoke = () =>
    revoke.mutate(
      { orgId, appId },
      {
        onSuccess: (saved) =>
          toast.success(`${saved.app_name} can no longer act in Oxygen. Its next call is refused.`),
        onError: (err) => toast.error(describeApiError(err, "Could not revoke the app's access."))
      }
    );

  return (
    <ListState
      what='app access'
      isPending={isPending}
      error={error}
      isEmpty={!data}
      onRetry={refetch}
      empty={null}
      testId='oxygen-access'
    >
      {data && (
        <div className='flex flex-col gap-4' data-testid='oxygen-access-approval'>
          <div className='flex items-center gap-2'>
            <ShieldCheck className='size-4 text-muted-foreground' />
            <h3 className='font-medium'>{data.app_name}</h3>
            <span className='text-xs text-muted-foreground'>{data.app_slug}</span>
          </div>

          <p className='text-sm text-muted-foreground'>
            {data.live_build
              ? `Its live build was published ${when(data.live_build.published_at)}${
                  data.live_build.published_by
                    ? ` by ${data.live_build.published_by}`
                    : data.live_build.published_via
                      ? ` by ${data.live_build.published_via}`
                      : ""
                }. Approving trusts whoever can publish this app.`
              : "This app has no live build."}
          </p>

          <div className='rounded-md bg-muted p-3 text-sm'>
            Oxygen checks the app, not the person using it. Anyone the app serves can do these
            through it; the app's own code decides who may ask. No key is created: the app acts in
            Oxygen directly, and every action is recorded in the audit log.
          </div>

          {data.actions.length === 0 ? (
            <p className='text-sm text-muted-foreground'>This app asks for nothing in Oxygen.</p>
          ) : (
            <ul className='flex flex-col gap-2'>
              {data.actions.map((a) => (
                <ActionRow key={a.action} action={a} />
              ))}
            </ul>
          )}

          <p className='text-xs text-muted-foreground'>
            {data.service_account && `Acts as the service account ${data.service_account}. `}
            {data.last_used_at
              ? `Last used ${when(data.last_used_at)}.`
              : data.token_id
                ? "Not used yet."
                : ""}
          </p>

          <div className='flex gap-2'>
            {pending.length > 0 && (
              <Button
                size='sm'
                onClick={onApprove}
                disabled={approve.isPending}
                data-testid='oxygen-access-approve'
              >
                Approve
              </Button>
            )}
            {data.token_id && (
              <Button
                size='sm'
                variant='outline'
                onClick={onRevoke}
                disabled={revoke.isPending}
                data-testid='oxygen-access-revoke'
              >
                Revoke
              </Button>
            )}
          </div>
        </div>
      )}
    </ListState>
  );
}
