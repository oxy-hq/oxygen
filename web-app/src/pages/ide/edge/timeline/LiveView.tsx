import { Radio } from "lucide-react";
import type React from "react";
import LivePlayer from "./LivePlayer";

/**
 * "Live" mode for the single-camera timeline. Drives the shared
 * [`LivePlayer`] for sub-second latency over WebRTC with HLS as
 * the fallback. There's deliberately no snapshot-polling fallback —
 * if both real-time transports fail, the player surfaces an explicit
 * error so the operator knows the box's Funnel isn't reachable.
 */
const LiveView: React.FC<{
  cameraId: string;
  cameraName: string | undefined;
}> = ({ cameraId, cameraName }) => (
  <div className='relative h-full w-full bg-black'>
    <LivePlayer cameraId={cameraId} className='absolute inset-0' />
    {/* "LIVE" badge — small, top-left, with a pulsing dot so the
        operator can tell at a glance this is live not a recording. */}
    <div className='absolute top-2 left-2 z-10 flex items-center gap-1.5 bg-black/60 px-2 py-1 text-white text-xs'>
      <Radio className='h-3 w-3 animate-pulse text-status-error' />
      <span className='font-semibold tracking-wide'>LIVE</span>
      {cameraName && <span className='text-white/80'>· {cameraName}</span>}
    </div>
  </div>
);

export default LiveView;
