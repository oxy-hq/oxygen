import { Check, Copy, Loader2 } from "lucide-react";
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
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue
} from "@/components/ui/shadcn/select";
import usePrepareDevice from "@/hooks/api/cameras/usePrepareDevice";
import useSites from "@/hooks/api/cameras/useSites";
import type { PreparedDeviceResponse } from "@/services/api/cameras";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

/**
 * "Add device" wizard (Option 3 from the operator Q&A).
 *
 * Two-step flow:
 *
 *   1. **Configure** — operator picks a site and (optionally)
 *      labels the hardware ("Jetson Orin Nano", "Intel N100",
 *      etc.). Submitting hits `/fleet/prepared-devices` which
 *      mints a fresh device identity + pre-creates the pending
 *      claim atomically.
 *   2. **Install** — backend returns an install snippet; the
 *      operator copies it into the new edge box's SSH session.
 *      The secret is shown ONCE — closing the dialog without
 *      copying means re-running the wizard.
 *
 * Why this exists instead of a shared install secret: per-install
 * credentials follow the Tailscale pre-auth / kubeadm bootstrap
 * pattern — if any one install token leaks, only that box is
 * compromised, never the workspace.
 */
type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

const AddDeviceDialog: React.FC<Props> = ({ open, onOpenChange }) => {
  const { workspace } = useCurrentWorkspace();
  const { data: sites = [] } = useSites(workspace?.id);
  const prepare = usePrepareDevice(workspace?.id);
  const [siteId, setSiteId] = useState<string>("");
  const [hardwareLabel, setHardwareLabel] = useState("");
  const [tsAuthkey, setTsAuthkey] = useState("");
  const [result, setResult] = useState<PreparedDeviceResponse | null>(null);
  const [copied, setCopied] = useState<"command" | "snippet" | null>(null);

  // Reset state on every open/close so reopening doesn't show
  // a stale snippet (which would also be a *misleading* snippet —
  // the previous secret is no longer recoverable).
  useEffect(() => {
    if (!open) {
      setSiteId("");
      setHardwareLabel("");
      setTsAuthkey("");
      setResult(null);
      setCopied(null);
    }
  }, [open]);

  // Auto-select the only site if there's exactly one.
  useEffect(() => {
    if (open && sites.length === 1 && !siteId) {
      setSiteId(sites[0].id);
    }
  }, [open, sites, siteId]);

  const handlePrepare = async () => {
    if (!siteId) {
      toast.error("Pick a site for the new box.");
      return;
    }
    const trimmedTsKey = tsAuthkey.trim();
    try {
      const out = await prepare.mutateAsync({
        siteId,
        hardwareLabel: hardwareLabel.trim() || undefined,
        tsAuthkey: trimmedTsKey || undefined
      });
      setResult(out);
    } catch (e) {
      const msg = e instanceof Error ? e.message : "Failed to prepare device.";
      toast.error(msg);
    }
  };

  const copyToClipboard = async (text: string, kind: "command" | "snippet") => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(kind);
      toast.success(`${kind === "command" ? "Install command" : "Manual snippet"} copied.`);
      setTimeout(() => setCopied(null), 2000);
    } catch {
      toast.error("Couldn't access the clipboard — select and copy manually.");
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={`${DESIGN_REFERENCE} max-w-2xl`}>
        <DialogHeader>
          <DialogTitle>{result ? "Run this on the new edge box" : "Add a device"}</DialogTitle>
          <DialogDescription>
            {result
              ? "The device secret is shown only once. Copy the snippet below into your edge box's SSH session, then start the worker."
              : "We'll mint a per-install identity and pre-create the claim. No shared install secret — the credentials below are for this one box only."}
          </DialogDescription>
        </DialogHeader>

        {result ? (
          <PreparedDeviceView result={result} copied={copied} onCopy={copyToClipboard} />
        ) : (
          <PrepareForm
            sites={sites}
            siteId={siteId}
            onSiteIdChange={setSiteId}
            hardwareLabel={hardwareLabel}
            onHardwareLabelChange={setHardwareLabel}
            tsAuthkey={tsAuthkey}
            onTsAuthkeyChange={setTsAuthkey}
            disabled={prepare.isPending}
          />
        )}

        <DialogFooter>
          {result ? (
            <Button onClick={() => onOpenChange(false)}>Done</Button>
          ) : (
            <>
              <Button
                variant='ghost'
                onClick={() => onOpenChange(false)}
                disabled={prepare.isPending}
              >
                Cancel
              </Button>
              <Button onClick={handlePrepare} disabled={prepare.isPending || sites.length === 0}>
                {prepare.isPending && <Loader2 className='mr-1.5 h-4 w-4 animate-spin' />}
                Generate
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

const PrepareForm: React.FC<{
  sites: { id: string; name: string }[];
  siteId: string;
  onSiteIdChange: (v: string) => void;
  hardwareLabel: string;
  onHardwareLabelChange: (v: string) => void;
  tsAuthkey: string;
  onTsAuthkeyChange: (v: string) => void;
  disabled: boolean;
}> = ({
  sites,
  siteId,
  onSiteIdChange,
  hardwareLabel,
  onHardwareLabelChange,
  tsAuthkey,
  onTsAuthkeyChange,
  disabled
}) => (
  <div className='flex flex-col gap-3'>
    <div className='flex flex-col gap-1'>
      <Label htmlFor='add-device-site'>Site</Label>
      <Select
        value={siteId}
        onValueChange={onSiteIdChange}
        disabled={disabled || sites.length === 0}
      >
        <SelectTrigger id='add-device-site'>
          <SelectValue placeholder={sites.length === 0 ? "No sites yet" : "Pick a site"} />
        </SelectTrigger>
        <SelectContent className={DESIGN_REFERENCE}>
          {sites.map((s) => (
            <SelectItem key={s.id} value={s.id}>
              {s.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
      {sites.length === 0 && (
        <span className='text-muted-foreground text-xs'>
          Create a site under the Sites tab first.
        </span>
      )}
    </div>
    <div className='flex flex-col gap-1'>
      <Label htmlFor='add-device-hardware'>Hardware label (optional)</Label>
      <Input
        id='add-device-hardware'
        value={hardwareLabel}
        onChange={(e) => onHardwareLabelChange(e.target.value)}
        placeholder='e.g. Jetson Orin Nano, Intel N100'
        disabled={disabled}
      />
      <span className='text-muted-foreground text-xs'>
        Free-text — shows up next to the device in the Fleet table.
      </span>
    </div>
    <div className='flex flex-col gap-1'>
      <Label htmlFor='add-device-ts-authkey'>Tailscale auth key (optional)</Label>
      <Input
        id='add-device-ts-authkey'
        type='password'
        value={tsAuthkey}
        onChange={(e) => onTsAuthkeyChange(e.target.value)}
        placeholder='tskey-auth-…'
        autoComplete='off'
        spellCheck={false}
        disabled={disabled}
        className='font-mono'
      />
      <span className='text-muted-foreground text-xs'>
        Pre-authorizes the box onto your tailnet so live preview works on first boot. Get a
        reusable, tagged key at{" "}
        <a
          href='https://login.tailscale.com/admin/settings/keys'
          target='_blank'
          rel='noreferrer'
          className='underline'
        >
          Tailscale → Settings → Keys
        </a>{" "}
        (tag <code className='font-mono'>tag:edge-box</code>). Skip if you'll set{" "}
        <code className='font-mono'>TS_AUTHKEY</code> on the box manually.
      </span>
    </div>
  </div>
);

const PreparedDeviceView: React.FC<{
  result: PreparedDeviceResponse;
  copied: "command" | "snippet" | null;
  onCopy: (text: string, kind: "command" | "snippet") => void;
}> = ({ result, copied, onCopy }) => {
  const [showFallback, setShowFallback] = useState(false);

  return (
    <div className='flex flex-col gap-3'>
      <div className='flex flex-col gap-1 border border-destructive/40 bg-destructive/5 p-2 text-xs'>
        <span className='font-medium text-destructive'>
          Secret shown once — save it or copy the command now.
        </span>
        <span className='text-muted-foreground'>
          Closing this dialog without copying means re-running the wizard to generate a new
          identity.
        </span>
      </div>

      <div className='flex flex-col gap-1'>
        <Label className='text-muted-foreground text-xs uppercase tracking-wide'>
          Run on the new edge box
        </Label>
        <div className='relative'>
          <pre className='max-h-40 overflow-y-auto whitespace-pre-wrap break-all border border-border bg-muted/40 p-3 pr-20 font-mono text-foreground text-xs'>
            {result.install_command}
          </pre>
          <Button
            size='sm'
            variant='outline'
            onClick={() => onCopy(result.install_command, "command")}
            className='absolute top-2 right-2'
            aria-label='Copy install command'
          >
            {copied === "command" ? (
              <>
                <Check className='mr-1.5 h-3.5 w-3.5' />
                Copied
              </>
            ) : (
              <>
                <Copy className='mr-1.5 h-3.5 w-3.5' />
                Copy
              </>
            )}
          </Button>
        </div>
        <span className='text-muted-foreground text-xs'>
          One-shot installer. Sets up docker, writes the identity, installs the systemd unit, and
          brings up the worker.
        </span>
      </div>

      <div className='flex flex-col gap-1'>
        <Button
          variant='ghost'
          size='sm'
          onClick={() => setShowFallback((v) => !v)}
          className='self-start text-muted-foreground text-xs'
        >
          {showFallback ? "Hide" : "Show"} airgapped fallback (manual JSON)
        </Button>
        {showFallback && (
          <div className='relative'>
            <pre className='max-h-60 overflow-y-auto whitespace-pre-wrap break-all border border-border bg-muted/40 p-3 pr-20 font-mono text-foreground text-xs'>
              {result.install_snippet}
            </pre>
            <Button
              size='sm'
              variant='outline'
              onClick={() => onCopy(result.install_snippet, "snippet")}
              className='absolute top-2 right-2'
              aria-label='Copy manual snippet'
            >
              {copied === "snippet" ? (
                <>
                  <Check className='mr-1.5 h-3.5 w-3.5' />
                  Copied
                </>
              ) : (
                <>
                  <Copy className='mr-1.5 h-3.5 w-3.5' />
                  Copy
                </>
              )}
            </Button>
          </div>
        )}
        {showFallback && (
          <span className='text-muted-foreground text-xs'>
            For boxes that can't reach the install script — writes the identity only. You'll still
            need docker + the compose stack + systemd unit yourself.
          </span>
        )}
      </div>

      <div className='flex flex-col gap-1 border border-border bg-muted/30 p-2 text-xs'>
        <span className='text-muted-foreground'>
          Device ID: <code className='break-all font-mono'>{result.device_id}</code>
        </span>
        <span className='text-muted-foreground'>
          Claim code: <code className='font-mono'>{result.claim_code}</code>
        </span>
      </div>
    </div>
  );
};

export default AddDeviceDialog;
