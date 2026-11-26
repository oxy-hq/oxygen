import { Loader2 } from "lucide-react";
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
import useCreateCamera from "@/hooks/api/cameras/useCreateCamera";
import useSites from "@/hooks/api/cameras/useSites";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Pre-selected site (e.g. when triggered from a site row). */
  defaultSiteId?: string | null;
};

/**
 * Manually add a camera by RTSP URL — for cameras Oxy hasn't
 * auto-discovered (i.e. anything outside UniFi today). Once an
 * edge box at the same site polls config, it picks the camera
 * up via the existing auto-claim path.
 *
 * V1 ships RTSP URL only; future iteration can add an ONVIF scan
 * driven by the bound edge box so the operator picks candidates
 * from a list instead of typing URLs.
 */
const AddCameraDialog: React.FC<Props> = ({ open, onOpenChange, defaultSiteId }) => {
  const { workspace } = useCurrentWorkspace();
  const { data: sites = [], isLoading: sitesLoading } = useSites(workspace?.id);
  const mutation = useCreateCamera(workspace?.id);

  const [siteId, setSiteId] = useState<string>("");
  const [name, setName] = useState("");
  const [rtspUrl, setRtspUrl] = useState("");
  const [substreamUrl, setSubstreamUrl] = useState("");

  // Default the site picker. If `defaultSiteId` is passed (from a
  // row-level "Add camera" button), use it; otherwise auto-pick the
  // first site so the form is one-click-submittable for the common
  // single-site case.
  useEffect(() => {
    if (!open) return;
    setName("");
    setRtspUrl("");
    setSubstreamUrl("");
    if (defaultSiteId) {
      setSiteId(defaultSiteId);
    } else if (sites.length > 0) {
      setSiteId(sites[0].id);
    } else {
      setSiteId("");
    }
  }, [open, defaultSiteId, sites]);

  const submitDisabled = !siteId || !name.trim() || !rtspUrl.trim() || mutation.isPending;

  const onSubmit = async () => {
    try {
      await mutation.mutateAsync({
        site_id: siteId,
        name: name.trim(),
        rtsp_url: rtspUrl.trim(),
        substream_url: substreamUrl.trim() || null
      });
      toast.success(`Camera "${name.trim()}" added`);
      onOpenChange(false);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Could not add camera";
      toast.error(msg);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={DESIGN_REFERENCE}>
        <DialogHeader>
          <DialogTitle>Add camera</DialogTitle>
          <DialogDescription>
            Paste the camera's RTSP URL. The edge box at the same site will pick the camera up on
            its next config poll (~30s).
          </DialogDescription>
        </DialogHeader>

        <div className='flex flex-col gap-3'>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='cam-site'>Site</Label>
            <Select value={siteId} onValueChange={setSiteId} disabled={sitesLoading}>
              <SelectTrigger id='cam-site'>
                <SelectValue
                  placeholder={
                    sitesLoading
                      ? "Loading sites…"
                      : sites.length === 0
                        ? "No sites — create one first"
                        : "Pick a site"
                  }
                />
              </SelectTrigger>
              <SelectContent className={DESIGN_REFERENCE}>
                {sites.map((s) => (
                  <SelectItem key={s.id} value={s.id}>
                    {s.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='cam-name'>Name</Label>
            <Input
              id='cam-name'
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder='Kitchen line'
              autoComplete='off'
            />
          </div>

          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='cam-rtsp'>RTSP URL</Label>
            <Input
              id='cam-rtsp'
              value={rtspUrl}
              onChange={(e) => setRtspUrl(e.target.value)}
              placeholder='rtsp://user:pass@cam.lan:554/Streaming/Channels/101'
              autoComplete='off'
              spellCheck={false}
              className='font-mono text-xs'
            />
            <span className='text-muted-foreground text-xs'>
              Embed credentials inline, or leave them out and configure them via the secrets store
              later.
            </span>
          </div>

          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='cam-substream'>Substream URL (optional)</Label>
            <Input
              id='cam-substream'
              value={substreamUrl}
              onChange={(e) => setSubstreamUrl(e.target.value)}
              placeholder='rtsp://user:pass@cam.lan:554/Streaming/Channels/102'
              autoComplete='off'
              spellCheck={false}
              className='font-mono text-xs'
            />
            <span className='text-muted-foreground text-xs'>
              Lower-resolution sub-stream for inference; saves bandwidth and GPU.
            </span>
          </div>
        </div>

        <DialogFooter>
          <Button
            variant='outline'
            onClick={() => onOpenChange(false)}
            disabled={mutation.isPending}
          >
            Cancel
          </Button>
          <Button onClick={onSubmit} disabled={submitDisabled}>
            {mutation.isPending ? (
              <>
                <Loader2 className='mr-2 h-4 w-4 animate-spin' />
                Adding…
              </>
            ) : (
              "Add camera"
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default AddCameraDialog;
