import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";
import {
  airhouseErrorMessage,
  useProvisionAirhouseTenant
} from "@/hooks/api/airhouse/useAdminAirhouse";

interface Props {
  workspaceId: string;
  workspaceName: string;
  orgName: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Confirms before provisioning a warehouse.
 *
 * Provisioning creates an external resource and permanently consumes a global
 * tenant name, and it is the row next to every unprovisioned workspace on a
 * dense list — the exact shape a misclick finds. The action is idempotent, so
 * the risk is not a double-provision; it is provisioning the wrong workspace,
 * which no retry undoes. Naming the workspace in the dialog is what makes that
 * checkable before it happens.
 */
export const ProvisionConfirmDialog = ({
  workspaceId,
  workspaceName,
  orgName,
  open,
  onOpenChange
}: Props) => {
  const provision = useProvisionAirhouseTenant(workspaceId);

  const handleConfirm = async (e: React.MouseEvent) => {
    e.preventDefault();
    try {
      await provision.mutateAsync();
      toast.success(`Warehouse provisioned for ${workspaceName}`);
      onOpenChange(false);
    } catch (err) {
      // The server's reason, not a generic one: a name collision (409), an
      // unconfigured deployment (503), and a warehouse-side fault are three
      // different next actions for the operator reading this toast.
      toast.error(airhouseErrorMessage(err, "Provisioning failed"));
    }
  };

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent
        className={DESIGN_REFERENCE}
        data-testid='admin-airhouse-provision-dialog'
      >
        <AlertDialogHeader>
          <AlertDialogTitle>Provision a warehouse for {workspaceName}?</AlertDialogTitle>
          <AlertDialogDescription>
            Creates an Airhouse tenant{orgName ? ` in ${orgName}` : ""} and claims its name. The
            name is global and cannot be released, so check the workspace before continuing.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={provision.isPending}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            onClick={handleConfirm}
            disabled={provision.isPending}
            data-testid='admin-airhouse-provision-confirm'
          >
            {provision.isPending ? "Provisioning…" : "Provision"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
};
