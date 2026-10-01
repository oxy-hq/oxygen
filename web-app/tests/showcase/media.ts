// Playwright films WebM from the moment the browser context opens. Slack plays
// H.264 MP4 inline, and the first second or two is a blank page loading — so
// trim to where the start page had settled and re-encode.

import { spawnSync } from "node:child_process";

function ffmpegAvailable(): boolean {
  return spawnSync("ffmpeg", ["-version"], { stdio: "ignore" }).status === 0;
}

/**
 * Trim and transcode `webm` to `mp4`. Returns the MP4 path, or undefined —
 * with the reason logged — when ffmpeg is missing or fails; the caller then
 * posts the screenshot alone.
 */
export function toMp4(webm: string, mp4: string, startMs: number): string | undefined {
  if (!ffmpegAvailable()) {
    console.warn("[showcase] ffmpeg is not installed — the video stays WebM and is not posted");
    return undefined;
  }
  const start = Math.max(0, startMs / 1000).toFixed(2);
  const res = spawnSync(
    "ffmpeg",
    [
      "-y",
      "-ss",
      start,
      "-i",
      webm,
      "-an",
      "-c:v",
      "libx264",
      "-pix_fmt",
      "yuv420p",
      "-preset",
      "veryfast",
      "-crf",
      "26",
      "-movflags",
      "+faststart",
      mp4
    ],
    { encoding: "utf-8" }
  );
  if (res.status !== 0) {
    console.warn(`[showcase] ffmpeg failed (${res.status}): ${res.stderr.slice(-400)}`);
    return undefined;
  }
  return mp4;
}
