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
import useCreateSite from "@/hooks/api/cameras/useCreateSite";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

/**
 * Manually create a site (no UniFi import). For operators who
 * either don't run UniFi or want to mix manual sites alongside
 * UniFi-discovered ones in the same workspace.
 *
 * Timezone defaults to the operator's browser timezone — most
 * common case is "the site is where I'm sitting." Operator can
 * override for remote sites.
 */
const AddSiteDialog: React.FC<Props> = ({ open, onOpenChange }) => {
  const { workspace } = useCurrentWorkspace();
  const mutation = useCreateSite(workspace?.id);

  const [name, setName] = useState("");
  const [timezone, setTimezone] = useState("");
  const [region, setRegion] = useState("");

  // Reset + default the timezone whenever the dialog opens.
  // `Intl.DateTimeFormat().resolvedOptions().timeZone` returns the
  // IANA name (e.g. "America/Los_Angeles") which matches what the
  // backend expects.
  useEffect(() => {
    if (!open) return;
    setName("");
    setTimezone(Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC");
    setRegion("");
  }, [open]);

  const onSubmit = async () => {
    const trimmed = name.trim();
    if (!trimmed) {
      toast.error("Name is required");
      return;
    }
    try {
      await mutation.mutateAsync({
        name: trimmed,
        timezone: timezone.trim() || "UTC",
        region: region.trim() || null
      });
      toast.success(`Site "${trimmed}" created`);
      onOpenChange(false);
    } catch (err) {
      const msg = err instanceof Error ? err.message : "Could not create site";
      toast.error(msg);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={DESIGN_REFERENCE}>
        <DialogHeader>
          <DialogTitle>Add site</DialogTitle>
          <DialogDescription>
            Create a site manually. Cameras and edge boxes are bound to one site each. Use the UniFi
            connector instead if you want sites auto-discovered.
          </DialogDescription>
        </DialogHeader>

        <div className='flex flex-col gap-3'>
          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='site-name'>Name</Label>
            <Input
              id='site-name'
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder='Pokehouse Soma'
              autoComplete='off'
              spellCheck={false}
            />
          </div>

          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='site-timezone'>Timezone</Label>
            <Input
              id='site-timezone'
              value={timezone}
              onChange={(e) => setTimezone(e.target.value)}
              placeholder='America/Los_Angeles'
              autoComplete='off'
              spellCheck={false}
              className='font-mono'
            />
            <span className='text-muted-foreground text-xs'>
              IANA timezone (e.g. America/Los_Angeles). Defaults to your browser's.
            </span>
          </div>

          <div className='flex flex-col gap-1.5'>
            <Label htmlFor='site-region'>Region (optional)</Label>
            <Input
              id='site-region'
              value={region}
              onChange={(e) => setRegion(e.target.value)}
              placeholder='us-west'
              autoComplete='off'
              spellCheck={false}
            />
            <span className='text-muted-foreground text-xs'>
              Free-form tag for rolling up dashboards across multiple sites.
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
          <Button onClick={onSubmit} disabled={mutation.isPending || !name.trim()}>
            {mutation.isPending ? (
              <>
                <Loader2 className='mr-2 h-4 w-4 animate-spin' />
                Creating…
              </>
            ) : (
              "Create site"
            )}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
};

export default AddSiteDialog;
