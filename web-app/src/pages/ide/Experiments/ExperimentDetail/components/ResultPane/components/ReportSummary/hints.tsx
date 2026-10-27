/** Plain-English meaning of each headline figure, shown on hover. */
export const HINTS = {
  estimate: (unit: string) =>
    `The average change in the target per ${unit} caused by switching the lever on, measured gross: before anything shifted away from sibling measures is taken off.`,
  interval:
    "The range of effects the data is consistent with. If it includes zero, the effect cannot be told apart from no effect at all.",
  confidence: (alphaWords: string) =>
    `Whether the result clears the confidence bar registered before the run (${alphaWords}). p is the chance a lever that did nothing would still show an effect this large; it is not the chance the effect is real.`,
  treatedTotal:
    "The per-unit effect added up over every treated unit and every day of the measured window, with its range. Arithmetic on what was measured, not a forecast.",
  treatedUnits: "How many units (or groups) the experiment switched on.",
  projection:
    "What the effect would add if it carried over to the units that were never treated. An assumption, not a measurement.",
  window: "The days after the switch that the effect was measured over.",
  compliance:
    "The share of treated units where the change actually landed. Below the floor, the number measures the attempt rather than the lever. Control leakage is the share of control units that got the change by mistake."
} as const;

export { Hinted, HintIcon } from "@/pages/ide/Experiments/components/glance/Hinted";
