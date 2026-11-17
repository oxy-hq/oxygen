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
import type { PublishToken } from "@/types/publishTokens";

interface RevokeTokenDialogProps {
  /** The token pending revocation, or null when the dialog is closed. */
  token: PublishToken | null;
  isRevoking: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}

export function RevokeTokenDialog({
  token,
  isRevoking,
  onOpenChange,
  onConfirm
}: RevokeTokenDialogProps) {
  return (
    <AlertDialog open={token !== null} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Revoke this token?</AlertDialogTitle>
          <AlertDialogDescription>
            “{token?.name}” will stop working immediately. Any CI using it will fail to publish
            until you issue a new token. This can't be undone.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={isRevoking}>Cancel</AlertDialogCancel>
          <AlertDialogAction
            disabled={isRevoking}
            onClick={(event) => {
              event.preventDefault();
              onConfirm();
            }}
          >
            {isRevoking ? "Revoking…" : "Revoke"}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
