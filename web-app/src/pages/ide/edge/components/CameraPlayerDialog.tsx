import { Bolt } from "lucide-react";
import type React from "react";
import { useCallback, useState } from "react";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Badge } from "@/components/ui/shadcn/badge";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import type { CameraSummary } from "@/services/api";
import LivePlayer, { type LiveStatus, type LiveTransport } from "../timeline/LivePlayer";

type Props = {
  camera: CameraSummary | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
};

/**
 * Live preview modal — thin wrapper around [`LivePlayer`]. The
 * player owns the WebRTC + HLS lifecycle (signaling, fallback,
 * teardown); this component only renders the dialog chrome and a
 * transport/status badge in the title so operators can tell low-
 * latency WebRTC apart from HLS.
 */
const CameraPlayerDialog: React.FC<Props> = ({ camera, open, onOpenChange }) => {
  const [status, setStatus] = useState<LiveStatus>("loading");
  const [transport, setTransport] = useState<LiveTransport>("webrtc");

  const onStatusChange = useCallback((s: LiveStatus, t: LiveTransport) => {
    setStatus(s);
    setTransport(t);
  }, []);

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className={`${DESIGN_REFERENCE} max-w-3xl`}>
        <DialogHeader>
          <DialogTitle className='flex items-center gap-2'>
            {camera?.name ?? "Camera"}
            {status === "playing" && (
              <Badge
                variant={transport === "webrtc" ? "default" : "secondary"}
                className='text-[10px]'
              >
                {transport === "webrtc" ? (
                  <>
                    <Bolt className='size-3' /> live · low latency
                  </>
                ) : (
                  <>live · HLS</>
                )}
              </Badge>
            )}
          </DialogTitle>
          <DialogDescription>
            {camera?.site_name}
            {camera?.edge_box_name && ` · ${camera.edge_box_name}`}
          </DialogDescription>
        </DialogHeader>

        <div className='relative flex aspect-video w-full items-center justify-center overflow-hidden border bg-muted/40'>
          {open && camera && (
            <LivePlayer
              cameraId={camera.id}
              onStatusChange={onStatusChange}
              className='absolute inset-0'
              controls
            />
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
};

export default CameraPlayerDialog;
