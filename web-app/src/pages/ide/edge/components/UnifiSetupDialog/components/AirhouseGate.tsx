import { AlertCircle } from "lucide-react";
import type React from "react";
import { useEffect } from "react";
import { useOpenSettings } from "@/components/settings/useSettingsRoute";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/shadcn/alert";
import { Button } from "@/components/ui/shadcn/button";
import { Spinner } from "@/components/ui/shadcn/spinner";
import useAirhouseConnection from "@/hooks/api/airhouse/useAirhouseConnection";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

type Props = {
  onReady: () => void;
};

/**
 * Step 0 of the UniFi wizard: Airhouse must be provisioned for this
 * workspace before camera events have anywhere to land. UniFi import
 * itself wouldn't fail without it (the soft `ensure_schema` on the
 * backend warn-and-continues), but events would silently no-op later.
 * Saved as project_camera_fleet_schema_gating in memory.
 *
 * Two terminal states:
 *   - provisioned → auto-advance to step 1 via `onReady`
 *   - not provisioned → render explanation + a button that switches
 *     the parent SettingsDialog to the Airhouse section
 */
const AirhouseGate: React.FC<Props> = ({ onReady }) => {
  const { workspace } = useCurrentWorkspace();
  const open = useOpenSettings();
  const { data, isLoading, error } = useAirhouseConnection(workspace?.id);

  const ready = !isLoading && !error && data?.is_provisioned === true;
  useEffect(() => {
    if (ready) {
      onReady();
    }
  }, [ready, onReady]);

  if (isLoading) {
    return (
      <div className='flex items-center justify-center py-8'>
        <Spinner />
      </div>
    );
  }

  if (error) {
    return (
      <Alert variant='destructive'>
        <AlertCircle />
        <AlertTitle>Couldn't check Airhouse status</AlertTitle>
        <AlertDescription>{error.message}</AlertDescription>
      </Alert>
    );
  }

  if (!data?.is_provisioned) {
    return (
      <div className='flex flex-col gap-4'>
        <Alert>
          <AlertCircle />
          <AlertTitle>Set up Airhouse first</AlertTitle>
          <AlertDescription>
            UniFi imports need an Airhouse warehouse to land camera events into. Provision Airhouse
            for this workspace, then come back and connect UniFi.
          </AlertDescription>
        </Alert>
        <div className='flex justify-end'>
          <Button onClick={() => open("workspace.airhouse")}>Go to Airhouse</Button>
        </div>
      </div>
    );
  }

  // Provisioned — onReady has fired via useEffect; show a brief
  // spinner while the parent transitions to step 1.
  return (
    <div className='flex items-center justify-center py-8'>
      <Spinner />
    </div>
  );
};

export default AirhouseGate;
