import Hls from "hls.js";
import { VideoOff } from "lucide-react";
import type React from "react";
import { useEffect, useState } from "react";
import { Spinner } from "@/components/ui/shadcn/spinner";
import { LOCAL_WORKSPACE_ID } from "@/libs/utils/constants";
import { CameraService } from "@/services/api";
import { apiBaseURL } from "@/services/env";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";

export type LiveTransport = "webrtc" | "hls";
export type LiveStatus = "loading" | "playing" | "unavailable";

type Props = {
  cameraId: string;
  /** Optional callback so parents (e.g. CameraPlayerDialog) can
   *  surface a transport/status badge in their own chrome. */
  onStatusChange?: (status: LiveStatus, transport: LiveTransport) => void;
  /** Tailwind classes applied to the outermost wrapper. Lets the
   *  caller decide between "aspect-video rounded card" and
   *  "fill the parent surface." */
  className?: string;
  /** Native HTML5 controls. Off by default — most surfaces in the
   *  IDE are kiosk-style. */
  controls?: boolean;
};

/**
 * How long we wait for the first media frame after WebRTC
 * negotiation completes before giving up and falling back to HLS.
 * 5s is generous — a healthy session has frames within ~500ms.
 */
const WEBRTC_FIRST_FRAME_TIMEOUT_MS = 5000;

/**
 * Safari WebRTC is a known second-class citizen: the
 * H.264-baseline-only constraint trips up most off-the-shelf
 * camera streams. Skip the WebRTC attempt entirely on Safari /
 * iOS to avoid the 5s timeout-then-fallback dance.
 */
function isSafari(): boolean {
  const ua = navigator.userAgent;
  return /^((?!chrome|android).)*safari/i.test(ua);
}

/**
 * Shared low-latency live player. Tries WebRTC first; falls back
 * to HLS on Safari, on signaling failure, or if no media arrives
 * within the timeout. If HLS also fails, surfaces an explicit
 * "live preview not available" state — we deliberately do NOT
 * fall back to snapshot polling.
 *
 * Two transport paths share the same `<video>` element:
 *   - WebRTC: browser ⇆ Tailscale Funnel ⇆ MediaMTX on the box.
 *     Oxy is in signaling only. ~200–500ms latency.
 *   - HLS: browser ⇆ Oxy proxy ⇆ MediaMTX on the box. Every byte
 *     through Oxy. ~5–10s latency.
 */
const LivePlayer: React.FC<Props> = ({ cameraId, onStatusChange, className, controls = false }) => {
  const { workspace } = useCurrentWorkspace();

  // Callback ref instead of `useRef` because surfaces like Radix
  // Dialog mount via a Portal — `useEffect` can fire one tick
  // before the portal commits. State-backed ref re-runs the
  // effect once the <video> element attaches.
  const [videoEl, setVideoEl] = useState<HTMLVideoElement | null>(null);
  const [status, setStatus] = useState<LiveStatus>("loading");
  const [transport, setTransport] = useState<LiveTransport>(() => (isSafari() ? "hls" : "webrtc"));

  useEffect(() => {
    onStatusChange?.(status, transport);
  }, [status, transport, onStatusChange]);

  // Reset transport whenever the camera changes so a successful
  // HLS fallback on camera A doesn't pre-empt the WebRTC attempt
  // on camera B. Done during render via the "reset state when prop
  // changes" pattern so biome doesn't strip cameraId from a useEffect
  // dep array (the effect body doesn't read cameraId; only the trigger
  // matters).
  const [lastCameraId, setLastCameraId] = useState(cameraId);
  if (cameraId !== lastCameraId) {
    setLastCameraId(cameraId);
    setTransport(isSafari() ? "hls" : "webrtc");
    setStatus("loading");
  }

  // WebRTC effect — only runs when we've chosen the WebRTC path.
  // Tearing down the RTCPeerConnection in the cleanup is essential;
  // a leaked pc keeps the box's coturn allocation alive until TTL.
  useEffect(() => {
    if (!videoEl || transport !== "webrtc") return;

    const effectiveWorkspaceId = workspace?.id ?? LOCAL_WORKSPACE_ID;
    let pc: RTCPeerConnection | null = null;
    let firstFrameTimer: number | undefined;
    let aborted = false;

    const fallbackToHls = (reason: string) => {
      if (aborted) return;
      console.warn("[LivePlayer] WebRTC failed, falling back to HLS:", reason);
      aborted = true;
      if (pc) {
        pc.close();
        pc = null;
      }
      setTransport("hls");
      setStatus("loading");
    };

    // Cannot reject: the whole body is inside the `try`, and its `catch` falls back to HLS.
    void (async () => {
      try {
        const session = await CameraService.requestWebrtcSession(effectiveWorkspaceId, cameraId);
        if (aborted) return;

        pc = new RTCPeerConnection({
          iceServers: session.ice_servers.map((s) => ({
            urls: s.urls,
            username: s.username,
            credential: s.credential
          }))
        });

        // Receive-only — operator just watches, no microphone uplink.
        pc.addTransceiver("video", { direction: "recvonly" });
        pc.addTransceiver("audio", { direction: "recvonly" });

        pc.ontrack = (e) => {
          if (aborted || !videoEl) return;
          videoEl.srcObject = e.streams[0];
        };

        // First successful media frame flips status. If it never
        // arrives within the timeout the fallback kicks in.
        const onLoadedData = () => {
          if (aborted) return;
          window.clearTimeout(firstFrameTimer);
          setStatus("playing");
        };
        videoEl.addEventListener("loadeddata", onLoadedData, { once: true });

        pc.oniceconnectionstatechange = () => {
          if (!pc) return;
          if (pc.iceConnectionState === "failed" || pc.iceConnectionState === "disconnected") {
            fallbackToHls(`ice state ${pc.iceConnectionState}`);
          }
        };

        const offer = await pc.createOffer();
        await pc.setLocalDescription(offer);

        // WHEP: POST the SDP offer as text/plain `application/sdp`,
        // get the answer back as the response body. Direct browser
        // -> Funnel call, NOT through apiClient — apiClient would
        // inject the Oxy bearer which the WHEP endpoint doesn't
        // know about and would reject.
        const res = await fetch(session.whep_url, {
          method: "POST",
          headers: { "Content-Type": "application/sdp" },
          body: offer.sdp
        });
        if (!res.ok) {
          fallbackToHls(`WHEP POST returned ${res.status}`);
          return;
        }
        const answerSdp = await res.text();
        if (aborted) return;

        await pc.setRemoteDescription({ type: "answer", sdp: answerSdp });

        firstFrameTimer = window.setTimeout(() => {
          if (videoEl?.readyState === 0) {
            fallbackToHls("no media within timeout");
          }
        }, WEBRTC_FIRST_FRAME_TIMEOUT_MS);
      } catch (err) {
        fallbackToHls(err instanceof Error ? err.message : "unknown error");
      }
    })();

    return () => {
      aborted = true;
      window.clearTimeout(firstFrameTimer);
      if (pc) pc.close();
      if (videoEl) videoEl.srcObject = null;
    };
  }, [cameraId, workspace?.id, videoEl, transport]);

  // HLS effect — runs after a WebRTC failure (or Safari direct).
  // If HLS also blows up we land on "unavailable" instead of
  // looping back to snapshot polling.
  useEffect(() => {
    if (!videoEl || transport !== "hls") return;

    const effectiveWorkspaceId = workspace?.id ?? LOCAL_WORKSPACE_ID;
    const url = `${apiBaseURL}/${effectiveWorkspaceId}/cameras/${cameraId}/preview/hls/index.m3u8`;
    const authToken = localStorage.getItem("auth_token");

    if (!Hls.isSupported()) {
      console.warn("[LivePlayer] Hls.isSupported() === false");
      setStatus("unavailable");
      return;
    }

    const hls = new Hls({
      xhrSetup: (xhr) => {
        if (authToken) xhr.setRequestHeader("Authorization", authToken);
      }
    });

    const onManifestParsed = () => setStatus("playing");
    const onError = (_event: unknown, data: { fatal?: boolean }) => {
      if (!data.fatal) return;
      setStatus("unavailable");
    };

    hls.on(Hls.Events.MANIFEST_PARSED, onManifestParsed);
    hls.on(Hls.Events.ERROR, onError);
    hls.attachMedia(videoEl);
    hls.loadSource(url);

    return () => {
      hls.off(Hls.Events.MANIFEST_PARSED, onManifestParsed);
      hls.off(Hls.Events.ERROR, onError);
      hls.destroy();
    };
  }, [cameraId, workspace?.id, videoEl, transport]);

  return (
    <div className={className ?? "relative h-full w-full bg-black"}>
      <video
        ref={setVideoEl}
        autoPlay
        muted
        playsInline
        className='h-full w-full bg-black object-contain'
        controls={controls && status === "playing"}
      />

      {status === "loading" && (
        <div className='absolute inset-0 flex items-center justify-center bg-background/70 backdrop-blur-sm'>
          <div className='flex flex-col items-center gap-2'>
            <Spinner />
            <p className='text-muted-foreground text-xs'>
              Connecting via {transport === "webrtc" ? "WebRTC" : "HLS"}…
            </p>
          </div>
        </div>
      )}

      {status === "unavailable" && (
        <div className='absolute inset-0 flex items-center justify-center bg-background/80 backdrop-blur-sm'>
          <div className='flex max-w-md flex-col items-center gap-2 text-center'>
            <VideoOff className='size-6 text-muted-foreground' />
            <p className='font-medium'>Live preview not available</p>
            <p className='text-muted-foreground text-xs'>
              This box hasn't reported a Tailscale Funnel hostname yet, so the browser can't reach
              its MediaMTX stream. Common in dev/OrbStack VMs without Funnel enabled — the rest of
              the pipeline (events, compliance reports, recordings) still works.
            </p>
            <p className='text-[10px] text-muted-foreground'>
              Set <code>EDGE_FUNNEL_HOSTNAME</code> on the box's <code>.env</code> and restart to
              enable live preview.
            </p>
          </div>
        </div>
      )}
    </div>
  );
};

export default LivePlayer;
