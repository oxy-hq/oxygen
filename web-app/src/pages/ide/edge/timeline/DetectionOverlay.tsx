import type React from "react";
import { useMemo } from "react";
import { cn } from "@/libs/shadcn/utils";
import type { PpeDetectionsEnvelope } from "@/services/api/cameras";

type Props = {
  envelope: PpeDetectionsEnvelope;
  /** Always show, regardless of clip currentTime. Useful in debug mode. */
  alwaysShow?: boolean;
};

/**
 * Renders PPE-YOLO bounding boxes over the ClipPlayer video.
 *
 * Coordinates are normalized [0,1] against the *source* frame YOLO ran
 * on. We render them as percentage offsets on a position:absolute layer
 * inside the same container as the `<video>`, which means CSS
 * automatically handles arbitrary container sizes — no manual
 * `getBoundingClientRect` math, and the bboxes stay in sync if the
 * operator resizes the player.
 *
 * Caveats this v0 explicitly does not solve:
 *   - **Letterbox/pillarbox drift.** The video element uses `object-fit:
 *     contain`-ish behavior, so when the container aspect ratio differs
 *     from the source frame's, the actual rendered video sits inside a
 *     letterboxed area. The bboxes here position against the full
 *     container, which means a slight offset against the real image
 *     in that case. Acceptable for v0 — most edge cameras are 16:9 and
 *     the player container is also 16:9.
 *   - **Per-frame tracking.** YOLO ran on a single frame; bboxes don't
 *     track moving subjects across the clip duration. The header
 *     timestamp ("YOLO @ 4.2s") makes the snapshot nature legible.
 *
 * VLM verdict remains authoritative. These overlays are visual hints
 * for reviewer triage.
 */
const DetectionOverlay: React.FC<Props> = ({ envelope, alwaysShow }) => {
  const detections = useMemo(
    () =>
      envelope.detections.map((d, i) => ({
        ...d,
        key: `${d.class}-${i}-${d.bbox.x.toFixed(3)}-${d.bbox.y.toFixed(3)}`
      })),
    [envelope.detections]
  );

  if (detections.length === 0) return null;

  return (
    <div
      className='pointer-events-none absolute inset-0'
      aria-hidden
      data-testid='detection-overlay'
    >
      {detections.map((d) => {
        const lowConfidence = d.confidence < 0.5;
        return (
          <div
            key={d.key}
            className={cn(
              "absolute border-2",
              lowConfidence
                ? "border-dot-warning/70 bg-dot-warning/5"
                : "border-dot-success/80 bg-dot-success/5"
            )}
            style={{
              left: `${d.bbox.x * 100}%`,
              top: `${d.bbox.y * 100}%`,
              width: `${d.bbox.width * 100}%`,
              height: `${d.bbox.height * 100}%`
            }}
          >
            <span
              className={cn(
                "absolute top-0 left-0 -translate-y-full whitespace-nowrap px-1 py-px font-medium font-mono text-[10px] text-white",
                lowConfidence ? "bg-dot-warning/90" : "bg-dot-success/90"
              )}
            >
              {d.class} {Math.round(d.confidence * 100)}%
            </span>
          </div>
        );
      })}
      {/* Snapshot timestamp pill — makes it obvious these bboxes are
          from one frame, not continuous tracking. Hidden when
          alwaysShow is set (debug mode usually doesn't need it). */}
      {!alwaysShow && (
        <span className='absolute top-2 right-2 bg-black/60 px-1.5 py-0.5 font-mono text-[10px] text-white'>
          YOLO @ {(envelope.frame_offset_ms / 1000).toFixed(1)}s · {envelope.model}
        </span>
      )}
    </div>
  );
};

export default DetectionOverlay;
