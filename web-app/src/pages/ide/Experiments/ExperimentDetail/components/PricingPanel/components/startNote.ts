import type { Grain } from "@/pages/ide/Experiments/grain";

/** Appended to a first-date hint: what the date must be at this grain. */
export function startNote(grain: Grain): string {
  if (grain === "hour") return " At hour grain, any whole hour.";
  if (grain === "week") return " It must be a Monday.";
  if (grain === "month") return " It must be the 1st of a month.";
  return "";
}
