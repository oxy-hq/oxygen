import { RefreshCw } from "lucide-react";
import type React from "react";
import { useMemo } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import { useQuickBooksConnect } from "@/hooks/api/quickbooks/useQuickBooksConnect";
import { useAirwayFiles } from "@/hooks/api/schedules/useSchedules";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { useRole } from "@/hooks/useRole";

/**
 * "Reconnect QuickBooks" button for a `quickbooks` airway pipeline. Reads
 * the client id + secret var names from the compiled pipeline list
 * (`GET /airway-pipelines`, any replica),
 * then re-runs the Intuit Connect flow to mint a fresh refresh token (Intuit
 * refresh tokens expire after ~100 days idle). Renders nothing when the
 * pipeline isn't a QuickBooks source.
 *
 * Surfaced next to a run error so an expired-token (`invalid_grant`) failure
 * has an obvious one-click fix.
 */
const QuickBooksReconnect: React.FC<{ pipelineRef: string }> = ({ pipelineRef }) => {
  const { project } = useCurrentProjectBranch();
  const { data: pipelines } = useAirwayFiles();
  const { connect, connecting } = useQuickBooksConnect(project.id);
  const { workspace: wsRole, is } = useRole();

  const qb = useMemo(() => {
    const source = pipelines?.find((p) => p.path === pipelineRef)?.quickbooks;
    return source
      ? {
          clientId: source.client_id,
          clientSecretVar: source.client_secret_var,
          refreshTokenVar: source.refresh_token_var
        }
      : null;
  }, [pipelines, pipelineRef]);
  if (!qb) return null;

  const onReconnect = async () => {
    try {
      const result = await connect({
        clientId: qb.clientId,
        // No secret value — reuse the stored client_secret_var.
        clientSecretVar: qb.clientSecretVar,
        refreshTokenVar: qb.refreshTokenVar,
        returnPath: window.location.href
      });
      if (result) {
        toast.success("QuickBooks reconnected", {
          description: "A fresh refresh token was stored. Re-run the pipeline."
        });
      }
    } catch {
      // hook already toasted
    }
  };

  // Hidden only for a role we KNOW is not admin. `useRole().workspace` is
  // `undefined` until workspace details load, and `CanWorkspaceAdmin` treats
  // undefined as deny — which would blank this button during the load window
  // and anywhere the role never populates. The point of the nit was that a
  // Member should not see a button that 403s; it was not to make the button
  // race a fetch.
  if (wsRole !== undefined && !is.workspaceAdmin) return null;

  return (
    <Button
      type='button'
      size='sm'
      variant='outline'
      onClick={onReconnect}
      disabled={connecting}
      data-testid='qb-reconnect-button'
    >
      <RefreshCw className='h-4 w-4' />
      {connecting ? "Reconnecting…" : "Reconnect QuickBooks"}
    </Button>
  );
};

export default QuickBooksReconnect;
