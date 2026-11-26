import { CameraOff, Loader2 } from "lucide-react";
import type React from "react";
import useCameraSnapshot from "@/hooks/api/cameras/useCameraSnapshot";
import { cn } from "@/libs/shadcn/utils";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

type Props = {
  cameraId: string;
  /** Polling cadence. Defaults match the snapshot proxy: 5s is the
   *  sweet spot between "feels live" and "doesn't hammer the edge". */
  intervalMs?: number;
  className?: string;
};

/**
 * Per-row camera thumbnail driven by the snapshot proxy
 * (`GET /{wid}/cameras/{cam_id}/preview/snapshot.jpg`).
 *
 * Three visual states:
 *   - `blobUrl`              → render the JPEG via `<img src>`.
 *   - loading first time     → spinner (placeholder dimension).
 *   - persistent error       → camera-off icon + "Preview unavailable"
 *     hover tooltip. Covers "no edge yet", "tailscale_ip not set",
 *     and transient upstream errors — all of which look the same to
 *     the operator at this zoom level.
 *
 * Sized at 96×54 (16:9 micro) to fit a table cell next to the camera
 * name without taking over the row.
 */
const CameraThumbnail: React.FC<Props> = ({ cameraId, intervalMs, className }) => {
  const { workspace } = useCurrentWorkspace();
  const { blobUrl, isLoading, isError } = useCameraSnapshot(workspace?.id, cameraId, intervalMs);

  const baseClasses = cn(
    // 16:9 micro thumbnail. `aspect-video` keeps the box at a
    // consistent ratio whatever the camera resolution; `w-24` (96px)
    // is the width the rest of the table looks balanced against.
    "flex aspect-video w-24 shrink-0 items-center justify-center overflow-hidden border bg-muted/40",
    className
  );

  if (blobUrl) {
    return (
      <div className={baseClasses}>
        <img
          src={blobUrl}
          alt='Camera preview'
          className='h-full w-full object-cover'
          draggable={false}
        />
      </div>
    );
  }

  if (isLoading) {
    return (
      <div className={baseClasses}>
        <Loader2 className='size-4 animate-spin text-muted-foreground' />
      </div>
    );
  }

  // Error / no-frame: same visual as "no edge connected".
  return (
    <div
      className={baseClasses}
      title={isError ? "Preview unavailable" : "Waiting for first frame"}
    >
      <CameraOff className='size-4 text-muted-foreground' />
    </div>
  );
};

export default CameraThumbnail;
