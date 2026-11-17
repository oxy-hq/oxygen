import { Check, Copy, Eye, EyeOff, TriangleAlert } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import type { CreatedPublishToken } from "@/types/publishTokens";

interface CreatedTokenDialogProps {
  /** The freshly created token (plaintext), or null when the dialog is closed. */
  token: CreatedPublishToken | null;
  onClose: () => void;
}

/**
 * Keep the recognizable prefix (through `oxypublish_` + a few chars) visible
 * and dot out the secret remainder, so the value is "slightly hidden" by
 * default but still identifiable. Reveal shows the full value; copy always
 * copies the full value regardless of reveal state.
 */
function maskSecret(value: string): string {
  return `${value.slice(0, 14)}${"•".repeat(16)}`;
}

/**
 * Shows a newly-minted token's plaintext **once**. The value is never
 * retrievable again (the server stores only a hash), so the reveal/copy
 * affordances and the "won't see it again" warning are the whole point of
 * this dialog. Masked by default to survive a shared screen / screenshare.
 */
export function CreatedTokenDialog({ token, onClose }: CreatedTokenDialogProps) {
  const [copied, setCopied] = useState(false);
  const [revealed, setRevealed] = useState(false);

  const copy = async () => {
    if (!token) return;
    try {
      await navigator.clipboard.writeText(token.token);
      setCopied(true);
      toast.success("Token copied to clipboard");
      setTimeout(() => setCopied(false), 2000);
    } catch {
      toast.error("Couldn't copy — reveal the value and copy manually");
    }
  };

  return (
    <Dialog
      open={token !== null}
      onOpenChange={(open) => {
        if (!open) {
          setCopied(false);
          setRevealed(false);
          onClose();
        }
      }}
    >
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Token created</DialogTitle>
          <DialogDescription>
            Copy this token now — it's shown once and can't be retrieved later. Store it as the
            <span className='font-mono'> OXY_TOKEN </span>
            secret in your CI.
          </DialogDescription>
        </DialogHeader>

        <div className='flex items-center gap-2 rounded-md border border-border bg-muted/40 p-2'>
          <code
            className={`flex-1 break-all font-mono text-xs ${
              revealed ? "select-all" : "select-none"
            }`}
          >
            {token ? (revealed ? token.token : maskSecret(token.token)) : ""}
          </code>
          <Button
            type='button'
            variant='ghost'
            size='icon'
            onClick={() => setRevealed((r) => !r)}
            aria-label={revealed ? "Hide token" : "Reveal token"}
          >
            {revealed ? <EyeOff className='size-4' /> : <Eye className='size-4' />}
          </Button>
          <Button
            type='button'
            variant='outline'
            size='icon'
            onClick={copy}
            aria-label='Copy token'
          >
            {copied ? <Check className='size-4' /> : <Copy className='size-4' />}
          </Button>
        </div>

        <p className='flex items-center gap-1.5 text-muted-foreground text-xs'>
          <TriangleAlert className='size-3.5' />
          Anyone with this token can publish custom apps on your behalf. Revoke it if it leaks.
        </p>

        <DialogFooter>
          <Button type='button' onClick={onClose}>
            Done
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
