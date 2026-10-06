import { Eye, EyeOff } from "lucide-react";
import { useState } from "react";
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
import { exportTokenSnippet } from "../utils/workflowSnippet";
import { CopyBlock } from "./CopyBlock";

interface TokenSecretDialogProps {
  /** The freshly minted or regenerated token. `null` keeps the dialog closed. */
  minted: TokenWithSecret | null;
  /** "created" or "regenerated": the verb the title uses. */
  verb: "created" | "regenerated";
  onClose: () => void;
}

const MASK = "•".repeat(32);

/**
 * The only time a secret is ever on screen. It can't be dismissed by a stray
 * click outside or Escape — closing it throws the secret away for good, so
 * that takes a deliberate press of "I've saved it".
 */
export function TokenSecretDialog({ minted, verb, onClose }: TokenSecretDialogProps) {
  const [revealed, setRevealed] = useState(false);

  const close = () => {
    setRevealed(false);
    onClose();
  };

  return (
    <Dialog open={minted !== null}>
      <DialogContent
        className='sm:max-w-lg'
        showCloseButton={false}
        onEscapeKeyDown={(e) => e.preventDefault()}
        onInteractOutside={(e) => e.preventDefault()}
        data-testid='api-access-secret-dialog'
      >
        <DialogHeader>
          <DialogTitle className='text-base'>
            {minted ? `${minted.token.name} ${verb}` : "Token"}
          </DialogTitle>
          <DialogDescription className='text-xs'>
            Copy the token now. It is shown this once and can't be recovered. If you lose it,
            regenerate it.
          </DialogDescription>
        </DialogHeader>

        {minted && (
          <div className='flex min-w-0 flex-col gap-4'>
            <div className='flex flex-col gap-1.5'>
              <div className='flex items-center justify-between'>
                <p className='font-medium text-xs'>Token</p>
                <Button
                  type='button'
                  variant='ghost'
                  size='sm'
                  className='h-6 gap-1 px-1.5 text-muted-foreground text-xs'
                  onClick={() => setRevealed((v) => !v)}
                  data-testid='api-access-secret-reveal'
                >
                  {revealed ? (
                    <EyeOff className='size-3' aria-hidden />
                  ) : (
                    <Eye className='size-3' aria-hidden />
                  )}
                  {revealed ? "Hide" : "Show"}
                </Button>
              </div>
              <CopyBlock
                text={minted.secret}
                display={revealed ? minted.secret : MASK}
                label='Copy token'
                testId='api-access-secret-value'
              />
            </div>

            <div className='flex flex-col gap-1.5'>
              <p className='font-medium text-xs'>Use it with oxyc</p>
              <CopyBlock
                text={exportTokenSnippet(minted.secret)}
                display={exportTokenSnippet(revealed ? minted.secret : MASK)}
                label='Copy shell command'
                testId='api-access-secret-export'
              />
              <p className='text-muted-foreground text-xs'>
                In CI, store it as a secret named <code className='font-mono'>OXY_TOKEN</code>{" "}
                instead of pasting it into a workflow file.
              </p>
            </div>
          </div>
        )}

        <DialogFooter>
          <Button size='sm' onClick={close} data-testid='api-access-secret-done'>
            I've saved it
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
