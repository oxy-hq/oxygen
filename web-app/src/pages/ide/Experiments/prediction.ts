import type { Prediction, PriorSource } from "@/types/experiment";

export type PredictionVerdict = "agrees" | "contradicts" | "unknown";

/**
 * The three-valued shape of the server's `prior_in_interval`, for styling and
 * for tests — NOT for wording: the sentence a reader sees is the server's own
 * `statement`, rendered verbatim and never recomposed from this. `null` means
 * the engine could not say (the earned set is unbounded, or a side of it is),
 * which is neither agreement nor disagreement.
 */
export function predictionVerdict(p: Pick<Prediction, "prior_in_interval">): PredictionVerdict {
  if (p.prior_in_interval === true) return "agrees";
  if (p.prior_in_interval === false) return "contradicts";
  return "unknown";
}

const SOURCE_LABEL: Record<PriorSource, string> = {
  declared: "declared in the World Model",
  fitted: "fitted from history"
};

/** Where the prior came from; a source this build does not know is shown as sent. */
export function priorSourceLabel(source: string): string {
  return SOURCE_LABEL[source as PriorSource] ?? source;
}
