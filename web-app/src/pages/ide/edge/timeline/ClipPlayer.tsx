import { Loader2, Radio } from "lucide-react";
import type React from "react";
import { useEffect, useRef, useState } from "react";
import { CameraService } from "@/services/api";
import type { PpeDetectionsEnvelope } from "@/services/api/cameras";
import DetectionOverlay from "./DetectionOverlay";
import { apiErrorMessage } from "./format";

/**
 * Native HTML5 video against a blob URL — clip is fetched once
 * through `apiClient` (carries the bearer + rides the Vite dev
 * proxy) and wrapped in `URL.createObjectURL`. Same trick as the
 * old ComplianceDetailPage player; see that file's comment for why
 * `<video src=…>` direct doesn't work with credentials mode.
 *
 * `onProgress(seconds)` fires on each `timeupdate` (every ~250ms in
 * Chrome) so the parent scrubber can render a moving playhead at
 * `start + seconds` on the wall-clock timeline.
 */
const ClipPlayer: React.FC<{
  workspaceId: string;
  cameraId: string;
  start: string;
  durationSeconds: number;
  onProgress?: (seconds: number) => void;
  /** When set, the error state renders a "Back to Live" button — used
   *  when the operator scrubbed onto a gap in recordings (common cause
   *  of 503) and needs an obvious way out of the dead-end. */
  onGoLive?: () => void;
  /** PPE-YOLO detection overlay (Option C). Only present when the
   *  playback target is a compliance report whose ingest carried
   *  detections; free-scrub clips never have one. */
  detections?: PpeDetectionsEnvelope | null;
}> = ({ workspaceId, cameraId, start, durationSeconds, onProgress, onGoLive, detections }) => {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [blobUrl, setBlobUrl] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    let createdUrl: string | null = null;
    setError(null);
    setBlobUrl(null);

    CameraService.fetchRecordingClip(workspaceId, cameraId, start, durationSeconds)
      .then((blob) => {
        if (cancelled) return;
        createdUrl = URL.createObjectURL(blob);
        setBlobUrl(createdUrl);
      })
      .catch((err) => {
        if (cancelled) return;
        setError(apiErrorMessage(err));
      });

    return () => {
      cancelled = true;
      if (createdUrl) URL.revokeObjectURL(createdUrl);
    };
  }, [workspaceId, cameraId, start, durationSeconds]);

  if (error) {
    // Scrubbing onto a gap in MTX recordings is the most common cause
    // of failures here (503 "no recording for the requested window").
    // Surface that as a calm "no footage here" rather than an error
    // banner, and give the operator a one-click exit.
    return (
      <div className='flex aspect-video w-full flex-col items-center justify-center gap-3 bg-black p-4 text-center text-muted-foreground text-sm'>
        <p className='max-w-md'>{error}</p>
        {onGoLive && (
          <button
            type='button'
            onClick={onGoLive}
            className='inline-flex items-center gap-1 border border-border bg-card px-3 py-1 text-foreground text-xs hover:bg-muted'
          >
            <Radio className='h-3 w-3' /> Back to Live
          </button>
        )}
      </div>
    );
  }

  if (!blobUrl) {
    return (
      <div className='flex aspect-video w-full items-center justify-center bg-black text-muted-foreground'>
        <Loader2 className='h-6 w-6 animate-spin' />
      </div>
    );
  }

  return (
    <div className='relative aspect-video w-full bg-black'>
      {/* biome-ignore lint/a11y/useMediaCaption: VLM clips have no caption track. */}
      <video
        ref={videoRef}
        src={blobUrl}
        controls
        autoPlay
        onTimeUpdate={
          onProgress ? (e) => onProgress((e.target as HTMLVideoElement).currentTime) : undefined
        }
        onEnded={onProgress ? () => onProgress(durationSeconds) : undefined}
        className='h-full w-full bg-black'
        data-testid='edge-clip-player'
      />
      {/* PPE-YOLO bbox overlay — only present when the playback target
          is a compliance report that carried detections. Positioned
          absolute over the video; see `DetectionOverlay` for the
          letterbox caveat. */}
      {detections && <DetectionOverlay envelope={detections} />}
    </div>
  );
};

export default ClipPlayer;
