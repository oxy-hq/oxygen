import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import type { TrustPolicy } from "@/types/orgApiAccess";
import { CopyBlock } from "../../../shared/CopyBlock";
import { buildWorkflowSnippet } from "../../../utils/workflowSnippet";

interface WorkflowSnippetDialogProps {
  /** The policy to write a workflow for. `null` keeps the dialog closed. */
  policy: TrustPolicy | null;
  /** True right after creating the policy, which changes the title. */
  justCreated: boolean;
  orgSlug: string;
  accountName: string;
  onClose: () => void;
}

/**
 * The other half of a trusted-access policy: the workflow that matches it.
 * Shown straight after creating one, because a policy with no workflow does
 * nothing, and reachable again from the row.
 */
export function WorkflowSnippetDialog({
  policy,
  justCreated,
  orgSlug,
  accountName,
  onClose
}: WorkflowSnippetDialogProps) {
  return (
    <Dialog open={policy !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className='sm:max-w-xl' data-testid='api-access-snippet-dialog'>
        <DialogHeader>
          <DialogTitle className='text-base'>
            {justCreated ? "Policy saved. Now add the workflow" : "Workflow for this policy"}
          </DialogTitle>
          <DialogDescription className='text-xs'>
            {policy && (
              <>
                Save this as <code className='font-mono'>{policy.workflow_path}</code> in{" "}
                <code className='font-mono'>{policy.repository}</code>. It needs no stored secret:
                each run trades GitHub's identity token for an Oxygen token that lasts 15 minutes.
              </>
            )}
          </DialogDescription>
        </DialogHeader>

        {policy && (
          <div className='flex min-w-0 flex-col gap-3'>
            <CopyBlock
              text={buildWorkflowSnippet({
                accountId: policy.service_account_id,
                orgSlug,
                accountName,
                workflowPath: policy.workflow_path,
                environment: policy.environment,
                refPattern: policy.ref_pattern,
                publishesApp: policy.grants.some((g) => g.kind === "app_publish" && !g.revoked_at)
              })}
              label='Copy workflow'
              testId='api-access-snippet'
              nowrap
            />
            {policy.environment ? (
              <p className='text-muted-foreground text-xs leading-relaxed'>
                The <code className='font-mono'>{policy.environment}</code> environment must exist
                in the repository's settings. Add required reviewers or a branch rule there to
                decide who can run this.
              </p>
            ) : (
              <p className='text-muted-foreground text-xs leading-relaxed'>
                This policy names no environment, so any run of this workflow matches it.
              </p>
            )}
          </div>
        )}

        <DialogFooter>
          <Button size='sm' onClick={onClose} data-testid='api-access-snippet-done'>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
