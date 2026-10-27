import { GitBranch, Loader2 } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import ErrorAlert from "@/components/ui/ErrorAlert";
import { Button } from "@/components/ui/shadcn/button";
import { useApplyWriteBack } from "@/hooks/api/useExperiments";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { useRole } from "@/hooks/useRole";
import { encodeBase64 } from "@/libs/encoding";
import ROUTES from "@/libs/utils/routes";
import useCurrentOrg from "@/stores/useCurrentOrg";
import useIdeBranch from "@/stores/useIdeBranch";
import type { WriteBackResponse } from "@/types/experiment";

/**
 * "Propose to semantic model": creates a branch carrying the coefficient patch
 * for a person to review and merge. The platform never writes main. Opening the
 * branch switches the IDE onto it first (as `useSaveToNewBranch` does after it
 * creates one), then lands on the edited file.
 */
export default function WriteBackAction({ name }: { name: string }) {
  const apply = useApplyWriteBack(name);
  const { is } = useRole();
  const { project } = useCurrentProjectBranch();
  const { setCurrentBranch } = useIdeBranch();
  const orgSlug = useCurrentOrg((s) => s.org?.slug) ?? "";
  const navigate = useNavigate();
  const [applied, setApplied] = useState<WriteBackResponse | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = async () => {
    setError(null);
    try {
      setApplied(await apply.mutateAsync());
    } catch (e) {
      setError(e instanceof Error ? e.message : "The branch could not be created.");
    }
  };

  const open = () => {
    if (!applied) return;
    setCurrentBranch(project.id, applied.branch);
    navigate(
      ROUTES.ORG(orgSlug).WORKSPACE(project.id).IDE.FILES.FILE(encodeBase64(applied.file_path))
    );
  };

  return (
    <div className='flex flex-col gap-2'>
      {!applied && !is.workspaceEditor && (
        <p className='text-muted-foreground text-xs' data-testid='experiment-writeback-denied'>
          Only a workspace editor or above can propose changes to the semantic model.
        </p>
      )}
      {!applied && is.workspaceEditor && (
        <Button
          variant='outline'
          size='sm'
          className='w-fit'
          disabled={apply.isPending}
          onClick={run}
          data-testid='experiment-writeback-apply'
        >
          {apply.isPending ? (
            <Loader2 className='h-4 w-4 animate-spin' />
          ) : (
            <GitBranch className='h-4 w-4' />
          )}
          Propose to semantic model
        </Button>
      )}
      {applied && (
        <div className='flex items-center gap-3 text-sm'>
          <span>
            Created branch{" "}
            <code className='font-mono' data-testid='experiment-writeback-branch'>
              {applied.branch}
            </code>
            .
          </span>
          <Button
            variant='outline'
            size='sm'
            onClick={open}
            data-testid='experiment-writeback-open'
          >
            Open branch
          </Button>
        </div>
      )}
      {error && (
        <div data-testid='experiment-writeback-error'>
          <ErrorAlert title='No branch was created' message={error} />
        </div>
      )}
    </div>
  );
}
