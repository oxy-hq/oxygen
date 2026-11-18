import { useEffect, useState } from "react";
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
import { Spinner } from "@/components/ui/shadcn/spinner";
import { useCreateSecret, useDeleteSecret } from "@/hooks/api/secrets/useSecretMutations";
import { useShareSecret } from "@/hooks/api/secrets/useSharedSecrets";
import useCurrentProjectBranch from "@/hooks/useCurrentProjectBranch";
import { apiErrorMessage } from "@/libs/apiError";
import { SecretService } from "@/services/secretService";
import type { AppSecretNeeds } from "@/types/sharedSecrets";
import { AppChecklist } from "./AppChecklist";

/** The app secret being lifted, as the table knows it. */
export interface LiftTarget {
  /** The stored secret's id — `apps/<app_id>/<KEY>`. */
  secretId: string;
  /** The bare key the app reads as `ctx.env.<key>`. */
  key: string;
  appId: string;
  appName: string;
}

/**
 * Turn one app's own secret into a workspace secret that several apps read.
 *
 * An `apps/<id>/<KEY>` secret is stored inside one app and nothing else can
 * reach it, so "share this with another app" has no direct form — the value has
 * to become a workspace secret first. This does that in one step: read the
 * value, store it as `KEY`, share it with the apps chosen, and remove the app's
 * own copy, which would otherwise keep winning over the shared one.
 */
export const LiftAppSecretDialog = ({
  target,
  apps,
  /** True when a workspace secret of this name already exists. */
  nameTaken,
  onOpenChange,
  onDone
}: {
  target: LiftTarget | null;
  apps: AppSecretNeeds[];
  nameTaken: boolean;
  onOpenChange: (open: boolean) => void;
  onDone: () => void;
}) => {
  const { project } = useCurrentProjectBranch();
  // Every step is silent: this dialog is the one voice for the whole move, and
  // it is the only thing that knows which of the four writes got through.
  const createSecret = useCreateSecret({ toastOnSuccess: false, toastOnError: false });
  const share = useShareSecret({ toastOnSuccess: false, toastOnError: false });
  const deleteSecret = useDeleteSecret({ toastOnSuccess: false, toastOnError: false });
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [running, setRunning] = useState(false);

  // Each target opens with its own app ticked: the lift must not quietly take
  // the key away from the app that has it today.
  // biome-ignore lint/correctness/useExhaustiveDependencies: reset on the target, not on every render's set
  useEffect(() => {
    setSelected(new Set(target ? [target.appId] : []));
  }, [target?.secretId]);

  if (!target) return null;

  // The owning app always reads the secret afterwards. Its box is ticked and
  // disabled, but the share is built from this set and not from the box, so
  // the guarantee does not depend on the UI holding the line.
  const readers = new Set([...selected, target.appId]);

  const lift = async () => {
    setRunning(true);
    // Which steps got far enough to leave something behind. The failure
    // message has to say what now exists, because the thing it creates lands
    // on the *other* tab, where the user is not looking.
    let createdId: string | null = null;
    let shared = false;
    try {
      const value = await SecretService.revealSecret(project.id, target.secretId);
      const created = await createSecret.mutateAsync({
        name: target.key,
        value,
        description: `Was ${target.appName}'s own ${target.key}`
      });
      createdId = created.id;
      await share.mutateAsync({ secretId: created.id, name: target.key, appIds: [...readers] });
      shared = true;
      // The app's own value wins over a shared one, so leaving it behind would
      // make the whole move a no-op for that app.
      await deleteSecret.mutateAsync(target.secretId);
      toast.success(
        `${target.key} is now a shared secret, read by ${readers.size} app${
          readers.size === 1 ? "" : "s"
        }`
      );
      onDone();
    } catch (error) {
      await reportFailure(error, createdId, shared);
    } finally {
      setRunning(false);
    }
  };

  /**
   * Say what the half-done move left behind, and undo what can be undone.
   *
   * A bare "nothing changed" is wrong here and traps the user: the shared
   * secret would already exist, so re-opening this dialog takes the
   * `nameTaken` branch, which offers no way to finish.
   */
  const reportFailure = async (error: unknown, createdId: string | null, shared: boolean) => {
    const title = `Couldn't make ${target.key} a shared secret`;
    // What the server said, when it said anything. It goes *after* the state,
    // never instead of it: `apiErrorMessage` prefers the server's sentence, and
    // "Request failed with status code 403" does not tell anyone what now
    // exists.
    const detail = apiErrorMessage(error, "");
    const say = (state: string) =>
      toast.error(title, { description: detail ? `${state} (${detail})` : state });

    // Nothing was written yet: the reveal or the create failed.
    if (!createdId) {
      say(`Nothing was changed — ${target.key} is still ${target.appName}'s own.`);
      return;
    }
    if (!shared) {
      // The value is stored but reaches nobody. Take it back, so the next try
      // starts from the same place this one did: left behind, it makes the
      // name taken, and this dialog would then offer no way to finish.
      try {
        await deleteSecret.mutateAsync(createdId);
        say(`It could not be shared, so the shared copy was removed. Nothing changed.`);
      } catch {
        say(
          `${target.key} was created as a shared secret but no app was given it, and it could not be removed. Share or delete it from the Shared tab.`
        );
      }
      return;
    }
    // Shared, but the app's own copy is still there — and it wins, so the move
    // has not taken effect yet. Say exactly that.
    say(
      `${target.key} is now a shared secret, but ${target.appName}'s own copy could not be removed — and an app's own value wins, so ${target.appName} still reads the old one. Delete it from the One app tab.`
    );
    onDone();
  };

  return (
    <Dialog open onOpenChange={onOpenChange}>
      <DialogContent className='sm:max-w-md' data-testid='lift-app-secret-dialog'>
        <DialogHeader>
          <DialogTitle className='break-all'>
            Share <code className='font-mono'>{target.key}</code> with more apps
          </DialogTitle>
          <DialogDescription>
            {nameTaken ? (
              <>
                A shared secret called <code className='font-mono'>{target.key}</code> already
                exists. Share that one from the Shared tab instead, then delete {target.appName}'s
                own copy.
              </>
            ) : (
              <>
                It is stored inside {target.appName} today, where nothing else can read it. Making
                it shared keeps one value and lets any app read it as{" "}
                <code className='font-mono'>ctx.env.{target.key}</code>.
              </>
            )}
          </DialogDescription>
        </DialogHeader>

        {!nameTaken && (
          <>
            <AppChecklist
              apps={apps}
              secretName={target.key}
              selected={selected}
              onChange={setSelected}
              locked={new Set([target.appId])}
              lockedReason={`${target.appName} owns this secret today and always keeps reading it.`}
            />
            <p className='text-muted-foreground text-xs'>
              {target.appName} keeps reading it, and its own copy is deleted — an app's own value
              wins over a shared one, so it would keep reading the old value otherwise.
            </p>
          </>
        )}

        <DialogFooter>
          <Button variant='outline' onClick={() => onOpenChange(false)} disabled={running}>
            {nameTaken ? "Close" : "Cancel"}
          </Button>
          {!nameTaken && (
            <Button onClick={lift} disabled={running} data-testid='lift-app-secret-confirm'>
              {running ? <Spinner /> : "Make shared"}
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};
