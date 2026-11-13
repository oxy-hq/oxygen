import { ArrowLeft } from "lucide-react";
import { useEffect, useState } from "react";
import GithubIcon from "@/components/ui/GithubIcon";
import { Button } from "@/components/ui/shadcn/button";
import { GitHubOnboardingStep } from "@/components/workspaces/components/CreateWorkspaceDialog/components/GitHubOnboardingStep";
import { OptionCard } from "@/components/workspaces/components/CreateWorkspaceDialog/components/OptionCard";
import type { WorkspaceCreationType } from "@/components/workspaces/types";
import useCurrentOrg from "@/stores/useCurrentOrg";
import WorkspacePreparing from "./WorkspacePreparing";

export type WorkspaceCreationPhase = "create" | "preparing";

type Mode = "pick" | "github";
type Created = { id: string; type: WorkspaceCreationType };

interface Props {
  /** Target org for creation. Defaults to the zustand current org — callers
   *  in a context where the current org isn't persisted (e.g. onboarding,
   *  where the org isn't yet the user's "active" one) must pass this
   *  explicitly. */
  org?: { id: string; slug: string };
  /** When omitted (no previous step to return to) the root "Back" button is
   *  hidden; the GitHub step's own "Back" to the picker remains. */
  onBack?: () => void;
  /** Fired once when the workspace has been created (before preparing runs).
   *  Useful for side effects like refetching a list. */
  onCreated?: (workspaceId: string, type: WorkspaceCreationType) => void;
  /** Fired whenever the internal phase toggles — for parent-owned headers. */
  onPhaseChange?: (phase: WorkspaceCreationPhase) => void;
}

/**
 * Full workspace creation flow — pick → submit → preparing — in one piece.
 * The only way in is a GitHub repository: a workspace exists only once a
 * repository backs it, so there is no demo or blank workspace.
 * Drives the preparing screen itself (which auto-navigates into the new
 * workspace when the backend reports ready); unmount the component to
 * cancel an in-progress preparing phase.
 */
export default function WorkspaceCreator({
  org: orgProp,
  onBack,
  onCreated,
  onPhaseChange
}: Props) {
  const storeOrg = useCurrentOrg((s) => s.org);
  const org = orgProp ?? (storeOrg ? { id: storeOrg.id, slug: storeOrg.slug } : null);
  const [mode, setMode] = useState<Mode>("pick");
  const [created, setCreated] = useState<Created | null>(null);

  useEffect(() => {
    onPhaseChange?.(created ? "preparing" : "create");
  }, [created, onPhaseChange]);

  const handleCreated = (workspaceId: string, type: WorkspaceCreationType) => {
    setCreated({ id: workspaceId, type });
    onCreated?.(workspaceId, type);
  };

  if (created && org) {
    return (
      <WorkspacePreparing
        workspaceId={created.id}
        creationType={created.type}
        orgId={org.id}
        orgSlug={org.slug}
        onRetry={() => {
          setCreated(null);
          setMode("pick");
        }}
      />
    );
  }

  if (mode === "github") {
    return (
      <GitHubOnboardingStep
        onBack={() => setMode("pick")}
        onDone={(workspaceId) => handleCreated(workspaceId, "github")}
      />
    );
  }

  return (
    <div className='flex flex-col gap-5'>
      <div className='flex flex-col gap-2'>
        <OptionCard
          label='01'
          icon={<GithubIcon className='h-3.5 w-3.5' />}
          title='Import from GitHub'
          description="Connect the organization's configuration repository."
          onClick={() => setMode("github")}
          testId='onboarding-github-workspace-card'
        />
      </div>

      {onBack && (
        <div className='flex items-center justify-start'>
          <Button variant='ghost' onClick={onBack}>
            <ArrowLeft className='size-3.5' />
            Back
          </Button>
        </div>
      )}
    </div>
  );
}
