// `@oxy-hq/sdk/ui`: the desk UI. The gallery's components, the recipes (`ui`), the tones and the
// icons, for a custom app's desk screens; the phone kit stays in `@oxy-hq/sdk/shell`.
//
// Styles: import "@oxy-hq/sdk/theme.css", "@oxy-hq/sdk/shell.css", then "@oxy-hq/sdk/ui.css"
// (every `.oxy-ui-*` rule, inside `@layer oxy-ui`). A Tailwind app also imports
// "@oxy-hq/sdk/tailwind.css" for the reference's utility names.
//
// One file per group folder, re-exported whole, so a group adds a piece by editing only its own
// `index.ts` (CONVENTIONS.md). React 19, no Tailwind, no runtime dependency.

export * from "./actions";
export * from "./charts";
export * from "./containers";
export { cx } from "./cx";
export * from "./feedback";
export * from "./figures";
export * from "./focus";
export * from "./forms";
export {
  ArrowSquareOutIcon,
  CaretDownIcon,
  CaretRightIcon,
  CheckIcon,
  CircleNotchIcon,
  Glyph,
  type IconProps,
  InfoIcon,
  PlusIcon,
  XIcon
} from "./icons";
export * from "./navigation";
export * from "./patterns";
export { type UiRecipes, ui } from "./recipes";
export * from "./tables";
export {
  type ChipTone,
  calloutTone,
  chipTone,
  phoneToneOf,
  present,
  STATUS_TONES,
  type StatusTone,
  statusToneOf,
  strongestTone,
  toneFill,
  toneText
} from "./tone";
