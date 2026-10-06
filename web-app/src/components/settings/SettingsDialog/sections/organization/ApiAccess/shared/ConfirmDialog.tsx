import { Loader2 } from "lucide-react";
import type { ReactNode } from "react";
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
import { cn } from "@/libs/shadcn/utils";

interface ConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  /** What will happen, in full. The person reads this instead of guessing. */
  children: ReactNode;
  /** The verb on the button, the same one the row action used. */
  confirmLabel: string;
  /** What Cancel says: "Keep token" reads better than "Cancel" beside "Revoke". */
  cancelLabel?: string;
  destructive?: boolean;
  isPending: boolean;
  onConfirm: () => void;
  testId: string;
}

/**
 * A confirmation that stays open while its request is in flight, so a failure
 * can be read and retried in place rather than after the dialog has vanished.
 */
export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  children,
  confirmLabel,
  cancelLabel = "Cancel",
  destructive = false,
  isPending,
  onConfirm,
  testId
}: ConfirmDialogProps) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent data-testid={testId}>
        <AlertDialogHeader>
          <AlertDialogTitle className='text-base'>{title}</AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className='flex flex-col gap-2 text-muted-foreground text-xs leading-relaxed'>
              {children}
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={isPending}>{cancelLabel}</AlertDialogCancel>
          <AlertDialogAction
            className={cn(destructive && "bg-destructive text-white hover:bg-destructive/90")}
            onClick={(e) => {
              // Radix closes on click; the caller closes on success instead.
              e.preventDefault();
              onConfirm();
            }}
            disabled={isPending}
            data-testid={`${testId}-confirm`}
          >
            {isPending && <Loader2 className='size-4 animate-spin' aria-hidden />}
            {confirmLabel}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
