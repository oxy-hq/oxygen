import { Copy, KeyRound } from "lucide-react";
import type React from "react";
import { toast } from "sonner";
import { CanWorkspaceAdmin } from "@/components/auth/Can";
import { Button } from "@/components/ui/shadcn/button";
import { useAuth } from "@/contexts/AuthContext";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import useSettingsDialog from "@/stores/useSettingsDialog";
import NoAccessNotice from "../../../components/NoAccessNotice";
import SectionHeader from "../../../components/SectionHeader";
import WorkspaceTokenTable from "./WorkspaceTokenTable";

/**
 * Workspace → API tokens: a read-only inventory of every API token that can act in this
 * workspace, the way to your own tokens, and the workspace id. Tokens are managed by their owner
 * under Account. Legacy API keys are not listed here: they have their own section,
 * Workspace → Legacy API keys.
 */
const ApiKeys: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  const { isLocalMode } = useAuth();
  const openSettings = useSettingsDialog((s) => s.open);

  const copyProjectId = async () => {
    if (!workspace?.id) return;
    try {
      await navigator.clipboard.writeText(workspace.id);
      toast.success("Copied to clipboard");
    } catch {
      toast.error("Failed to copy to clipboard");
    }
  };

  return (
    <CanWorkspaceAdmin
      fallback={<NoAccessNotice>You need workspace admin access to see API tokens.</NoAccessNotice>}
    >
      <div className='flex flex-col gap-5' data-testid='settings-api-keys'>
        <SectionHeader
          icon={KeyRound}
          title='API tokens'
          description='Every API token that can act in this workspace, whoever owns it. Each owner manages their own from their account.'
          actions={
            // Local mode has no accounts, so there is no Account section to open.
            !isLocalMode && (
              <Button
                size='sm'
                variant='outline'
                onClick={() => openSettings("account.tokens")}
                data-testid='workspace-tokens-manage-link'
              >
                Manage your tokens
              </Button>
            )
          }
        />

        <WorkspaceTokenTable />

        <div className='space-y-2'>
          <p className='text-muted-foreground text-xs'>Workspace ID, for API calls</p>
          <div className='flex items-center gap-2'>
            <div className='flex h-8 flex-1 items-center rounded-md border bg-background px-3 font-mono text-xs'>
              {workspace?.id ?? "—"}
            </div>
            <Button
              variant='outline'
              size='sm'
              onClick={copyProjectId}
              aria-label='Copy workspace ID'
            >
              <Copy className='h-4 w-4' />
            </Button>
          </div>
        </div>
      </div>
    </CanWorkspaceAdmin>
  );
};

export default ApiKeys;
