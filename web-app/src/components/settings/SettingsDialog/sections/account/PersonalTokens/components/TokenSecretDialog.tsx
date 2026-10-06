import { Copy } from "lucide-react";
import type React from "react";
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
import type { TokenWithSecret } from "@/types/apiToken";
import { isFixedToken } from "../accessSummary";

export interface SecretReveal extends TokenWithSecret {
  /** Which action produced the secret: it changes the heading and nothing else. */
  reason: "created" | "regenerated";
}

interface Props {
  /** `null` keeps the dialog closed. The secret lives only in the parent's state. */
  reveal: SecretReveal | null;
  onDone: () => void;
}

/** The shell line that puts the token where oxyc and the SDKs look for it. */
export const exportSnippet = (secret: string): string => `export OXY_TOKEN=${secret}`;

const copy = async (text: string, what: string) => {
  try {
    await navigator.clipboard.writeText(text);
    toast.success(`Copied ${what}`);
  } catch (error) {
    console.error("Failed to copy to clipboard:", error);
    toast.error("Couldn't copy. Select the text and copy it by hand.");
  }
};

interface CopyRowProps {
  label: string;
  value: string;
  /** Names the thing in the toast: "the token", "the export line". */
  what: string;
  testId: string;
}

/** One selectable line of code with its own copy button. */
const CopyRow: React.FC<CopyRowProps> = ({ label, value, what, testId }) => (
  <div className='flex flex-col gap-1.5'>
    <p className='font-medium text-xs'>{label}</p>
    <div className='flex items-stretch gap-2'>
      <code
        className='min-w-0 flex-1 select-all break-all rounded-md border bg-muted/50 px-3 py-2 font-mono text-xs'
        data-testid={testId}
      >
        {value}
      </code>
      <Button
        type='button'
        variant='outline'
        size='sm'
        className='h-auto shrink-0'
        onClick={() => copy(value, what)}
        aria-label={`Copy ${what}`}
        data-testid={`${testId}-copy`}
      >
        <Copy />
        Copy
      </Button>
    </div>
  </div>
);

/**
 * The one time a token's secret is shown, after create or regenerate. Closing takes an explicit
 * click: a stray click outside or Escape would throw away something that can't be shown again.
 */
const TokenSecretDialog: React.FC<Props> = ({ reveal, onDone }) => (
  <Dialog open={reveal !== null} onOpenChange={(open) => !open && onDone()}>
    <DialogContent
      className='sm:max-w-xl'
      showCloseButton={false}
      onInteractOutside={(event) => event.preventDefault()}
      onEscapeKeyDown={(event) => event.preventDefault()}
      data-testid='account-token-secret-dialog'
    >
      {reveal && (
        <>
          <DialogHeader>
            <DialogTitle className='truncate'>
              {reveal.reason === "created"
                ? `Copy your new token, ${reveal.token.name}`
                : `Copy the new secret for ${reveal.token.name}`}
            </DialogTitle>
            <DialogDescription className='text-xs'>
              {/* A sandbox agent token can't be regenerated: a lost one is revoked and replaced. */}
              {isFixedToken(reveal.token)
                ? "You won't see this again. Oxygen stores only a hash of it, so if it's lost the fix is to revoke this token and create another."
                : "You won't see this again. Oxygen stores only a hash of it, so if it's lost the fix is to regenerate the token."}
              {reveal.reason === "regenerated" && " The previous secret has stopped working."}
            </DialogDescription>
          </DialogHeader>

          <div className='flex min-w-0 flex-col gap-4'>
            <CopyRow
              label='Token'
              value={reveal.secret}
              what='the token'
              testId='account-token-secret'
            />
            <CopyRow
              label={
                isFixedToken(reveal.token)
                  ? "Or set it in the agent's environment"
                  : "Or set it in your shell or CI"
              }
              value={exportSnippet(reveal.secret)}
              what='the export line'
              testId='account-token-export'
            />
          </div>

          <DialogFooter>
            <Button size='sm' onClick={onDone} data-testid='account-token-secret-done'>
              I've copied it
            </Button>
          </DialogFooter>
        </>
      )}
    </DialogContent>
  </Dialog>
);

export default TokenSecretDialog;
