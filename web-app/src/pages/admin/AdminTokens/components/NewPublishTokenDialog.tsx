import { Loader2 } from "lucide-react";
import { type FormEvent, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import { useCreatePublishToken } from "@/hooks/api/publishTokens/usePublishTokens";
import type { CreatedPublishToken } from "@/types/publishTokens";

/**
 * Naming a new publish token. It was a form card pinned to the top of its own page;
 * on a page that lists every kind of token it is the one kind staff mint here (sandbox
 * tokens are minted by approving an agent, staff tokens by `oxyc login`), so it is a
 * header action that opens this, and the created token's one-time reveal follows.
 */
export function NewPublishTokenDialog({
  open,
  onOpenChange,
  onCreated
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: (token: CreatedPublishToken) => void;
}) {
  const create = useCreatePublishToken();
  const [name, setName] = useState("");

  const onSubmit = (e: FormEvent<HTMLFormElement>) => {
    e.preventDefault();
    create.mutate(name.trim(), {
      onSuccess: (token) => {
        setName("");
        onOpenChange(false);
        onCreated(token);
      }
    });
  };

  return (
    <Dialog open={open} onOpenChange={(next) => !create.isPending && onOpenChange(next)}>
      <DialogContent className='sm:max-w-md'>
        <form onSubmit={onSubmit} className='space-y-4'>
          <DialogHeader>
            <DialogTitle>New publish token</DialogTitle>
            <DialogDescription>
              For <span className='font-mono'>oxyc publish</span> in CI, set as the{" "}
              <span className='font-mono'>OXY_TOKEN</span> secret. It can publish and read custom
              apps only, and works until revoked.
            </DialogDescription>
          </DialogHeader>
          <Input
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder='Token name, e.g. ci-publish'
            aria-label='Token name'
            disabled={create.isPending}
            autoComplete='off'
            autoFocus
            data-testid='admin-tokens-new-name'
          />
          <DialogFooter>
            <Button
              type='submit'
              disabled={create.isPending || !name.trim()}
              data-testid='admin-tokens-new-submit'
            >
              {create.isPending ? (
                <>
                  <Loader2 className='size-4 animate-spin' />
                  Creating…
                </>
              ) : (
                "Create token"
              )}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
