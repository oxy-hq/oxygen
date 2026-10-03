import { AlertTriangle } from "lucide-react";
import type React from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import type { Secret } from "@/types/secret";

interface DeleteSecretDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  secret: Secret | null;
  onConfirm: () => void;
  /** The delete is in flight: a second click would send a second request. */
  isDeleting?: boolean;
}

export const DeleteSecretDialog: React.FC<DeleteSecretDialogProps> = ({
  open,
  onOpenChange,
  secret,
  onConfirm,
  isDeleting = false
}) => {
  if (!secret) {
    return null;
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-[425px]'>
        <DialogHeader>
          <DialogTitle className='flex items-center gap-2'>
            <AlertTriangle className='h-5 w-5 text-destructive' />
            Delete Secret
          </DialogTitle>
          <DialogDescription>
            This action cannot be undone. This will permanently delete the secret.
          </DialogDescription>
        </DialogHeader>

        <div className='py-4'>
          <div className='rounded-lg bg-muted p-4'>
            <p className='font-medium text-sm'>Deleting secret:</p>
            <p className='mt-1 text-muted-foreground text-sm'>{secret.name}</p>
            {secret.description && (
              <>
                <p className='mt-3 font-medium text-sm'>Description:</p>
                <p className='mt-1 text-muted-foreground text-sm'>{secret.description}</p>
              </>
            )}
          </div>

          <div className='mt-4 rounded-lg border border-warning/20 bg-warning/5 p-4'>
            <div className='flex items-start gap-2'>
              <AlertTriangle className='mt-0.5 h-4 w-4 flex-shrink-0 text-warning' />
              <div className='text-sm'>
                <p className='font-medium text-warning'>Warning</p>
                <p className='mt-1 text-muted-foreground'>
                  Any configurations using this secret will lose access and may stop functioning
                  properly. Make sure to update all references before deleting.
                </p>
              </div>
            </div>
          </div>
        </div>

        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant='destructive' onClick={onConfirm} disabled={isDeleting}>
            Delete Secret
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
