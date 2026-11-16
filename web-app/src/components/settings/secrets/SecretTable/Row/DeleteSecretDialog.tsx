import { Loader2 } from "lucide-react";
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
import type { Secret } from "@/types/secret";

interface DeleteSecretDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  secret: Secret | null;
  /** The bare key apps read, when the stored name is `apps/<id>/<KEY>`. */
  displayName?: string;
  /** The apps that read this secret today — named, so the cost is specific. */
  readers?: string[];
  onConfirm: () => void;
  /** The delete is in flight: a second click would send a second request. */
  isDeleting?: boolean;
}

/** "a, b and c", so the sentence reads. */
const list = (names: string[]) =>
  names.length <= 1
    ? (names[0] ?? "")
    : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;

/**
 * Confirm deleting a secret.
 *
 * It names the apps that read it, because "configurations using this secret may
 * stop functioning" is not something anyone can act on — the question is always
 * *which* ones. An `AlertDialog`, like every other destructive confirmation in
 * settings, and it stays open while the request is in flight so a failure can be
 * read and retried in place.
 */
export const DeleteSecretDialog: React.FC<DeleteSecretDialogProps> = ({
  open,
  onOpenChange,
  secret,
  displayName,
  readers = [],
  onConfirm,
  isDeleting = false
}) => {
  if (!secret) {
    return null;
  }

  const name = displayName ?? secret.name;
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent data-testid='delete-secret-dialog'>
        <AlertDialogHeader>
          <AlertDialogTitle className='break-all text-base'>
            Delete <span className='font-mono'>{name}</span>?
          </AlertDialogTitle>
          <AlertDialogDescription asChild>
            <div className='flex flex-col gap-2 text-muted-foreground text-xs leading-relaxed'>
              <p>
                The value is erased and cannot be recovered. Anything reading it gets nothing on its
                next call — storing the secret again later means pasting the value in again.
              </p>
              {readers.length > 0 ? (
                <p>
                  <span className='text-foreground'>{list(readers)}</span>{" "}
                  {readers.length === 1 ? "reads" : "read"} it as{" "}
                  <code className='font-mono text-foreground'>ctx.env.{name}</code>.
                </p>
              ) : (
                <p>No app reads it. The workspace's databases, integrations and agents may.</p>
              )}
              {secret.description && <p className='border-l-2 pl-3'>{secret.description}</p>}
            </div>
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={isDeleting}>Keep secret</AlertDialogCancel>
          <AlertDialogAction
            className='bg-destructive text-white hover:bg-destructive/90'
            onClick={(e) => {
              // Radix closes on click; the caller closes on success instead.
              e.preventDefault();
              onConfirm();
            }}
            disabled={isDeleting}
            data-testid='delete-secret-confirm'
          >
            {isDeleting && <Loader2 className='size-4 animate-spin' aria-hidden />}
            Delete secret
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
};
