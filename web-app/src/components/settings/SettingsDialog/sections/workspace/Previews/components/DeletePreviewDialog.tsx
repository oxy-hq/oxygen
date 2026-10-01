import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogDestructiveAction,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle
} from "@/components/ui/shadcn/alert-dialog";

interface Props {
  branch: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
  isPending: boolean;
}

export default function DeletePreviewDialog({
  branch,
  open,
  onOpenChange,
  onConfirm,
  isPending
}: Props) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent className='bg-popover sm:max-w-md'>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete preview?</AlertDialogTitle>
          <AlertDialogDescription>
            Links to the <code className='font-mono'>{branch}</code> preview stop working. The
            branch itself is untouched, and you can create the preview again at any time.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={isPending}>Cancel</AlertDialogCancel>
          <AlertDialogDestructiveAction
            onClick={(e) => {
              // Stay open until the delete settles, so a failure is not hidden
              // behind a dialog that already closed.
              e.preventDefault();
              onConfirm();
            }}
            disabled={isPending}
            data-testid='delete-preview-confirm'
          >
            {isPending ? "Deleting…" : "Delete preview"}
          </AlertDialogDestructiveAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
