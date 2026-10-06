import type React from "react";
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
import { buttonVariants } from "@/components/ui/shadcn/utils/button-variants";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The key's name, as its row shows it. */
  name: string;
  onConfirm: () => void;
}

/** Asks before revoking: a revoked legacy API key cannot be extended back to life. */
const RevokeLegacyKeyDialog: React.FC<Props> = ({ open, onOpenChange, name, onConfirm }) => (
  <AlertDialog open={open} onOpenChange={onOpenChange}>
    <AlertDialogContent className='bg-popover sm:max-w-md'>
      <AlertDialogHeader>
        <AlertDialogTitle>Revoke {name}?</AlertDialogTitle>
        <AlertDialogDescription>
          Anything using this legacy API key stops working at once, and it can't be brought back.
        </AlertDialogDescription>
      </AlertDialogHeader>
      <AlertDialogFooter>
        <AlertDialogCancel>Cancel</AlertDialogCancel>
        <AlertDialogAction
          onClick={onConfirm}
          className={buttonVariants({ variant: "destructive" })}
          data-testid='legacy-api-key-revoke-confirm'
        >
          Revoke
        </AlertDialogAction>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
);

export default RevokeLegacyKeyDialog;
