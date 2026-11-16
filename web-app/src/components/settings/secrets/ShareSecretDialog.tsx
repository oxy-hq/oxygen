import { useEffect, useState } from "react";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useShareSecret } from "@/hooks/api/secrets/useSharedSecrets";
import type { AppSecretNeeds } from "@/types/sharedSecrets";
import { AppChecklist } from "./AppChecklist";

const sameSet = (a: Set<string>, b: Set<string>) =>
  a.size === b.size && [...a].every((x) => b.has(x));

/**
 * Choose which of the workspace's apps read one workspace secret. Saving sends
 * the whole set: an app left unticked stops reading it on its next call.
 */
export const ShareSecretDialog = ({
  secret,
  apps,
  ownValue,
  onOpenChange,
  onCreateFor
}: {
  /** The workspace secret, and the apps it is shared with now. `null` closes. */
  secret: { id: string; name: string; appIds: string[] } | null;
  apps: AppSecretNeeds[];
  /** Apps that store their own value of this key. */
  ownValue?: Set<string>;
  onOpenChange: (open: boolean) => void;
  /** Offered when the workspace has no apps yet: nothing to share with. */
  onCreateFor?: (name: string, appIds: string[]) => void;
}) => {
  const share = useShareSecret();
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const initial = new Set(secret?.appIds ?? []);

  // A different secret (or a reopen) starts from what is shared now.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset on the secret, not on every render's set
  useEffect(() => {
    setSelected(new Set(secret?.appIds ?? []));
  }, [secret?.id]);

  if (!secret) return null;
  const save = () =>
    share.mutate(
      { secretId: secret.id, name: secret.name, appIds: [...selected] },
      { onSuccess: () => onOpenChange(false) }
    );

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-md' data-testid='share-secret-dialog'>
        <DialogHeader>
          <DialogTitle className='break-all'>
            Which apps read <code className='font-mono'>{secret.name}</code>?
          </DialogTitle>
          <DialogDescription>
            Each one reads it as <code className='font-mono'>ctx.env.{secret.name}</code>. It stays
            one value: change it here and every app gets the new one on its next call. Unticking an
            app stops it reading the secret on its next call.
          </DialogDescription>
        </DialogHeader>
        <AppChecklist
          apps={apps}
          secretName={secret.name}
          selected={selected}
          onChange={setSelected}
          ownValue={ownValue}
        />
        {apps.length === 0 && onCreateFor && (
          <p className='text-muted-foreground text-xs'>
            Publish an app and it will appear here. The secret keeps its value in the meantime — the
            workspace's databases, integrations and agents already read it.
          </p>
        )}
        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            onClick={save}
            disabled={share.isPending || sameSet(selected, initial)}
            data-testid='share-secret-save'
          >
            {share.isPending ? <Spinner /> : "Save"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
