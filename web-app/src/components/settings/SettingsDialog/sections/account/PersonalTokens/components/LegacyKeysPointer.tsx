import type React from "react";
import { Button } from "@/components/ui/shadcn/button";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import useSettingsDialog from "@/stores/useSettingsDialog";

/**
 * One quiet line under the token table: where the older kind of key is now. This list holds
 * tokens only, so someone looking for a key they made before needs the way there.
 *
 * Shown to everyone who has a workspace loaded, whatever their role: Workspace → Legacy API keys
 * is open to every member, since a legacy API key belongs to its owner. With no workspace there
 * is no Workspace group in the nav, and the link would land somewhere else.
 */
const LegacyKeysPointer: React.FC = () => {
  const hasWorkspace = useCurrentWorkspace((s) => !!s.workspace);
  const openSettings = useSettingsDialog((s) => s.open);

  if (!hasWorkspace) return null;

  return (
    <p className='text-muted-foreground text-xs' data-testid='account-token-legacy-pointer'>
      Older keys are under{" "}
      <Button
        variant='link'
        className='h-auto p-0 font-normal text-muted-foreground text-xs underline underline-offset-2 hover:text-foreground'
        onClick={() => openSettings("workspace.legacy_api_keys")}
        data-testid='account-token-legacy-link'
      >
        Workspace → Legacy API keys
      </Button>
      .
    </p>
  );
};

export default LegacyKeysPointer;
