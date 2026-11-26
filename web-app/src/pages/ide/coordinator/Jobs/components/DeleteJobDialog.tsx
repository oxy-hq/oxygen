import type React from "react";
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
import { buttonVariants } from "@/components/ui/shadcn/utils/button-variants";
import type { Schedule } from "@/types/schedule";

interface Props {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  schedule: Schedule | null;
  onConfirm: () => void;
}

/** Confirm-and-audit gate before a job definition is permanently removed. */
const DeleteJobDialog: React.FC<Props> = ({ open, onOpenChange, schedule, onConfirm }) => (
  <AlertDialog open={open} onOpenChange={onOpenChange}>
    <AlertDialogContent className={`${DESIGN_REFERENCE} bg-popover sm:max-w-md`}>
      <AlertDialogHeader>
        <AlertDialogTitle>Delete job</AlertDialogTitle>
        <AlertDialogDescription>
          Delete "{schedule?.name}"? This stops all future runs for this job. In-flight runs are
          unaffected. This cannot be undone.
        </AlertDialogDescription>
      </AlertDialogHeader>
      <AlertDialogFooter>
        <AlertDialogCancel>Cancel</AlertDialogCancel>
        <AlertDialogAction
          onClick={onConfirm}
          className={buttonVariants({ variant: "destructive" })}
        >
          Delete
        </AlertDialogAction>
      </AlertDialogFooter>
    </AlertDialogContent>
  </AlertDialog>
);

export default DeleteJobDialog;
