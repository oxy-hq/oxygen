import { Copy } from "lucide-react";
import { QRCodeSVG } from "qrcode.react";
import { toast } from "sonner";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { Input } from "@/components/ui/shadcn/input";
import type { CreatedKioskDevice } from "@/types/frontline";

/**
 * The one moment this enrol link exists on screen. The server keeps only a
 * hash of the token, so closing it loses the link; the way back is a new link
 * from the kiosk's row, which kills this one. No guard on close — losing the
 * link costs one click, not the kiosk.
 */
export function EnrolLinkDialog({
  device,
  onClose
}: {
  device: CreatedKioskDevice | null;
  onClose: () => void;
}) {
  const copyLink = async () => {
    if (!device) return;
    try {
      await navigator.clipboard.writeText(device.enrol_url);
      toast.success("Enroll link copied");
    } catch (error) {
      console.error("Failed to copy enroll link:", error);
      toast.error("Couldn't copy — select the link and copy it by hand");
    }
  };

  return (
    <Dialog open={device !== null} onOpenChange={(open) => !open && onClose()}>
      <DialogContent className='sm:max-w-md'>
        <DialogHeader>
          <DialogTitle>Enroll {device?.name}</DialogTitle>
          <DialogDescription>
            Scan this code with the tablet, or open the link on it. It works once, for 24 hours.
            Lost it? Make a new link from the kiosk's row.
          </DialogDescription>
        </DialogHeader>
        {/* Rendered locally: the URL carries the enrol token, so it never goes to a QR service.
            Dark-on-white in both themes — scanners misread an inverted code. */}
        <div className='flex justify-center pt-1'>
          <div className='rounded-md bg-white p-3' data-testid='settings-crew-enrol-qr'>
            <QRCodeSVG
              value={device?.enrol_url ?? ""}
              size={192}
              level='M'
              title='Enroll link QR code'
            />
          </div>
        </div>
        <div className='flex items-center gap-2 pt-1'>
          <Input
            readOnly
            value={device?.enrol_url ?? ""}
            onFocus={(e) => e.currentTarget.select()}
            className='font-mono text-xs'
            aria-label='Enroll link'
            data-testid='settings-crew-enrol-link'
          />
          <Button
            type='button'
            size='sm'
            className='shrink-0 gap-1.5'
            onClick={copyLink}
            data-testid='settings-crew-enrol-link-copy'
          >
            <Copy className='h-4 w-4' />
            Copy link
          </Button>
        </div>
        <div className='flex justify-end'>
          <Button type='button' variant='outline' size='sm' onClick={onClose}>
            Done
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  );
}
