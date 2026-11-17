/**
 * The publish pipeline's stage timings, as `oxyc publish` prints them.
 *
 * The server times each stage of a publish (`admit → inflate → store → record →
 * promote → point → after`) and answers the timings on success as `stages`; a
 * refused publish names the stage it was refused at in the `x-oxy-publish-stage`
 * header. Servers older than either send neither, and nothing is printed.
 */

/** The response header naming the stage a refused publish failed at. */
export const PUBLISH_STAGE_HEADER = "x-oxy-publish-stage";

/** One stage's wall time, as the server answers it. */
export interface StageTiming {
  stage: string;
  ms: number;
}

/** `12ms` under a second, `1.2s` under ten, whole seconds past that. */
export function formatMs(ms: number): string {
  const n = Number.isFinite(ms) ? Math.max(0, ms) : 0;
  if (n < 1000) return `${Math.round(n)}ms`;
  const s = n / 1000;
  return s < 10 ? `${s.toFixed(1)}s` : `${Math.round(s)}s`;
}

/**
 * `admit 12ms · inflate 340ms · … (total 1.6s)`, in the order the server sent
 * them; `undefined` when it sent none.
 */
export function formatStages(stages: StageTiming[] | undefined): string | undefined {
  if (!stages?.length) return undefined;
  const total = stages.reduce((sum, s) => sum + Math.max(0, s.ms), 0);
  const each = stages.map((s) => `${s.stage} ${formatMs(s.ms)}`).join(" · ");
  return `${each} (total ${formatMs(total)})`;
}

/** The refused publish's headline, naming its stage when the server did. */
export function refusalHeadline(status: number, stage: string | null | undefined): string {
  const at = stage?.trim();
  return at ? `publish failed (${status}) at the ${at} stage` : `publish failed (${status})`;
}
