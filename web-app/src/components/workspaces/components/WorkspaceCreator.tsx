import { ArrowLeft, BookOpen, Loader2, Plus } from "lucide-react";
import { useEffect, useState } from "react";
import GithubIcon from "@/components/ui/GithubIcon";
import { Button } from "@/components/ui/shadcn/button";
import { Input } from "@/components/ui/shadcn/input";
import { Label } from "@/components/ui/shadcn/label";
import { GitHubOnboardingStep } from "@/components/workspaces/components/CreateWorkspaceDialog/components/GitHubOnboardingStep";
import { OptionCard } from "@/components/workspaces/components/CreateWorkspaceDialog/components/OptionCard";
import type { WorkspaceCreationType } from "@/components/workspaces/types";
import { extractErrorMessage } from "@/components/workspaces/utils";
import { OnboardingService } from "@/services/api/onboarding";
import useCurrentOrg from "@/stores/useCurrentOrg";
import WorkspacePreparing from "./WorkspacePreparing";

export type WorkspaceCreationPhase = "create" | "preparing";

type Mode = "pick" | "new" | "github" | "submitting";
type Created = { id: string; type: WorkspaceCreationType };

interface Props {
  /** Target org for creation. Defaults to the zustand current org — callers
   *  in a context where the current org isn't persisted (e.g. onboarding,
   *  where the org isn't yet the user's "active" one) must pass this
   *  explicitly. */
  org?: { id: string; slug: string };
  /** When omitted (no previous step to return to) the root "Back" button is
   *  hidden; the sub-form "Back" buttons that exit github/new back to pick
   *  remain since those are purely internal. */
  onBack?: () => void;
  /** Fired once when the workspace has been created (before preparing runs).
   *  Useful for side effects like refetching a list. */
  onCreated?: (workspaceId: string, type: WorkspaceCreationType) => void;
  /** Fired whenever the internal phase toggles — for parent-owned headers. */
  onPhaseChange?: (phase: WorkspaceCreationPhase) => void;
}

/**
 * Full workspace creation flow — pick → submit → preparing — in one piece.
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
  const [workspaceName, setWorkspaceName] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [created, setCreated] = useState<Created | null>(null);

  useEffect(() => {
    onPhaseChange?.(created ? "preparing" : "create");
  }, [created, onPhaseChange]);

  const handleCreated = (workspaceId: string, type: WorkspaceCreationType) => {
    setCreated({ id: workspaceId, type });
    onCreated?.(workspaceId, type);
  };

  const handleDemo = async () => {
    if (!org?.id) {
      setError("Select an organization first.");
      return;
    }
    setMode("submitting");
    setError(null);
    try {
      const result = await OnboardingService.setupDemo(org.id);
      handleCreated(result.workspace_id, "demo");
    } catch (err) {
      setError(extractErrorMessage(err) ?? "Failed to set up demo workspace");
      setMode("pick");
    }
  };

  const handleNewCreate = async () => {
    if (!org?.id) {
      setError("Select an organization first.");
      return;
    }
    setMode("submitting");
    setError(null);
    try {
      const result = await OnboardingService.setupNew(org.id, workspaceName.trim() || undefined);
      handleCreated(result.workspace_id, "new");
    } catch (err) {
      setError(extractErrorMessage(err) ?? "Failed to create workspace");
      setMode("new");
    }
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

  if (mode === "submitting") {
    return (
      <div className='flex flex-col items-center gap-3 py-12'>
        <Loader2 className='size-5 animate-spin text-primary' />
        <p className='text-muted-foreground text-sm'>Setting up workspace…</p>
      </div>
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

  if (mode === "new") {
    return (
      <div className='flex flex-col gap-4'>
        <div className='space-y-1.5'>
          <Label htmlFor='new-ws-name'>Workspace name</Label>
          <Input
            id='new-ws-name'
            data-testid='onboarding-workspace-name-input'
            placeholder='my-workspace'
            value={workspaceName}
            onChange={(e) => setWorkspaceName(e.target.value)}
            onKeyDown={(e) => {
              // handleNewCreate cannot reject: it catches the failure and shows it inline.
              if (e.key === "Enter") void handleNewCreate();
            }}
            autoFocus
          />
          <p className='text-muted-foreground text-xs'>Leave blank for a default name.</p>
        </div>

        {error && (
          <p className='rounded-md border border-destructive/20 bg-destructive/5 px-3 py-2 text-center text-destructive text-sm'>
            {error}
          </p>
        )}

        <div className='flex gap-2'>
          <Button
            variant='outline'
            className='flex-1'
            onClick={() => {
              setError(null);
              setMode("pick");
            }}
          >
            Back
          </Button>
          <Button
            className='flex-1'
            onClick={handleNewCreate}
            data-testid='onboarding-create-workspace-button'
          >
            Create workspace
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className='flex flex-col gap-5'>
      <div className='flex flex-col gap-2'>
        <OptionCard
          label='01'
          icon={<GithubIcon className='h-3.5 w-3.5' />}
          title='Import from GitHub'
          description='Clone an existing repository and start working immediately.'
          badge='Recommended'
          onClick={() => setMode("github")}
          testId='onboarding-github-workspace-card'
        />
        <OptionCard
          label='02'
          icon={<BookOpen className='h-3.5 w-3.5' />}
          title='Demo Workspace'
          description='Explore Oxygen with pre-built sample data and example queries.'
          onClick={handleDemo}
          testId='onboarding-demo-workspace-card'
        />
        <OptionCard
          label='03'
          icon={<Plus className='h-3.5 w-3.5' />}
          title='Blank Workspace'
          description='Start from scratch with an empty workspace.'
          onClick={() => {
            setWorkspaceName("");
            setError(null);
            setMode("new");
          }}
          testId='onboarding-blank-workspace-card'
        />
      </div>

      {error && (
        <p className='rounded-md border border-destructive/20 bg-destructive/5 px-3 py-2 text-center text-destructive text-sm'>
          {error}
        </p>
      )}

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
