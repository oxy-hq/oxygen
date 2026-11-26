import { CheckCircle2, KeyRound, Loader2, Search, Trash2 } from "lucide-react";
import type React from "react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
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
import { Label } from "@/components/ui/shadcn/label";
import {
  useDeleteUnifiCredential,
  useSetUnifiCredential,
  useUnifiCredentialStatus
} from "@/hooks/api/cameras/useUnifiCredentialStatus";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import UnifiSetupDialog from "./UnifiSetupDialog";

/**
 * Inline status bar that surfaces the workspace's UniFi credential
 * state above the Sites table. Lets the operator save the key once
 * and skip every subsequent inline-paste prompt across the import,
 * resync, and bulk-rewrite flows.
 *
 * Lives in SitesTable because that's where the operator notices the
 * absence: the table is dominated by source="unifi" rows once a few
 * customers are onboarded, and "is my key still saved?" is the
 * question operators ask first when something breaks.
 */
const UnifiCredentialBanner: React.FC = () => {
  const { workspace } = useCurrentWorkspace();
  const { data: status, isLoading } = useUnifiCredentialStatus(workspace?.id);
  const setMutation = useSetUnifiCredential(workspace?.id);
  const deleteMutation = useDeleteUnifiCredential(workspace?.id);

  const [dialogOpen, setDialogOpen] = useState(false);
  const [apiKey, setApiKey] = useState("");
  // Drives the Scan-sites UnifiSetupDialog in `scan-stored` mode. Opens
  // from the explicit "Scan sites" button on the configured banner, and
  // also auto-opens right after saving a fresh key so the operator sees
  // what they just connected without an extra click.
  const [scanDialogOpen, setScanDialogOpen] = useState(false);

  useEffect(() => {
    if (!dialogOpen) setApiKey("");
  }, [dialogOpen]);

  const onSave = async () => {
    const trimmed = apiKey.trim();
    if (!trimmed) {
      toast.error("Paste a UniFi API key first");
      return;
    }
    try {
      await setMutation.mutateAsync({ apiKey: trimmed });
      toast.success("UniFi credential saved");
      setDialogOpen(false);
      // Surface the scan flow immediately — the operator just told us
      // about a new key, the natural next question is "what's in it?".
      setScanDialogOpen(true);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not save credential");
    }
  };

  const onDelete = async () => {
    try {
      await deleteMutation.mutateAsync();
      toast.success("UniFi credential cleared");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Could not clear credential");
    }
  };

  if (isLoading) {
    return (
      <div className='flex items-center gap-2 border border-border bg-muted/30 px-3 py-2 text-muted-foreground text-xs'>
        <Loader2 className='h-3 w-3 animate-spin' />
        Checking UniFi credential…
      </div>
    );
  }

  const configured = status?.configured === true;
  return (
    <div
      className={
        configured
          ? "flex items-center justify-between gap-3 border border-dot-success/40 bg-dot-success/5 px-3 py-2"
          : "flex items-center justify-between gap-3 border border-dot-warning/40 bg-dot-warning/5 px-3 py-2"
      }
    >
      <div className='flex items-center gap-2 text-xs'>
        {configured ? (
          <>
            <CheckCircle2 className='h-3.5 w-3.5 text-status-success' />
            <span className='font-medium'>UniFi credential configured</span>
            {status?.updated_at && (
              <span className='text-muted-foreground'>
                · last set {new Date(status.updated_at).toLocaleDateString()}
              </span>
            )}
          </>
        ) : (
          <>
            <KeyRound className='h-3.5 w-3.5 text-status-warning' />
            <span className='font-medium'>No UniFi credential saved</span>
            <span className='text-muted-foreground'>
              · save once to skip the inline key prompt on import + resync
            </span>
          </>
        )}
      </div>
      <div className='flex items-center gap-1.5'>
        {configured && (
          <Button
            size='sm'
            variant='outline'
            onClick={() => setScanDialogOpen(true)}
            className='h-7 text-xs'
            title='Re-check the UniFi account for new sites and cameras using the stored credential.'
          >
            <Search className='mr-1 h-3 w-3' />
            Scan sites
          </Button>
        )}
        <Button
          size='sm'
          variant='outline'
          onClick={() => setDialogOpen(true)}
          className='h-7 text-xs'
        >
          {configured ? "Replace" : "Configure"}
        </Button>
        {configured && (
          <Button
            size='sm'
            variant='ghost'
            onClick={onDelete}
            disabled={deleteMutation.isPending}
            className='h-7 text-xs'
            title='Clear the stored credential — the inline key prompt returns on the next import or resync.'
          >
            {deleteMutation.isPending ? (
              <Loader2 className='h-3 w-3 animate-spin' />
            ) : (
              <Trash2 className='h-3 w-3' />
            )}
          </Button>
        )}
      </div>

      <UnifiSetupDialog open={scanDialogOpen} onOpenChange={setScanDialogOpen} mode='scan-stored' />

      <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
        <DialogContent className={`${DESIGN_REFERENCE} max-w-md`}>
          <DialogHeader>
            <DialogTitle>UniFi credential</DialogTitle>
            <DialogDescription>
              Stored encrypted in the workspace secret store. The UniFi import, resync, and bulk
              RTSP rewrite endpoints all read from this when no key is passed inline. Replace
              anytime; clearing deletes the secret entirely.
            </DialogDescription>
          </DialogHeader>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='unifi-api-key'>API key</Label>
            <Input
              id='unifi-api-key'
              type='password'
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder='ui_xxx…'
              autoComplete='off'
              spellCheck={false}
              className='font-mono'
            />
            <span className='text-muted-foreground text-xs'>
              Get one at{" "}
              <a href='https://unifi.ui.com' target='_blank' rel='noreferrer' className='underline'>
                unifi.ui.com
              </a>{" "}
              → Settings → API Keys (top-right user menu).
            </span>
          </div>
          <DialogFooter>
            <Button
              variant='outline'
              onClick={() => setDialogOpen(false)}
              disabled={setMutation.isPending}
            >
              Cancel
            </Button>
            <Button onClick={onSave} disabled={setMutation.isPending || apiKey.trim() === ""}>
              {setMutation.isPending ? (
                <>
                  <Loader2 className='mr-1.5 h-3.5 w-3.5 animate-spin' />
                  Saving…
                </>
              ) : (
                "Save credential"
              )}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
};

export default UnifiCredentialBanner;
