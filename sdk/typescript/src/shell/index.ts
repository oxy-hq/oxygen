// @oxy-hq/sdk/shell — the Oxygen workspace chrome (48px icon rail +
// universal top bar) as reusable components. The main web-app renders these
// same components, so a custom app that mounts them reads as one product.
//
// Two layers:
// - Presentational (ShellRail, TopBar, Breadcrumb, …): props only, no
//   router, no fetching. The web-app composes these with its own stores.
// - Wired (OxyShell + useShellContext): full frame for custom-app
//   bundles, fed by the bundle-gated `/api/projects/:id/shell-context`
//   endpoint. Must be mounted inside <OxyAppProvider>.
//
// Styling: import "@oxy-hq/sdk/shell.css" once. The stylesheet is
// namespaced (`oxy-shell-*`) and needs no Tailwind; it follows the host's
// design tokens when present and falls back to the Oxygen defaults. Dark
// mode = a `.dark` class on any ancestor.

export type { AnswerChartProps, ChartBlock, ChartBlockConfig } from "./AnswerChart";
export { AnswerChart } from "./AnswerChart";
export type { AskDockProps } from "./AskDock";
export { AskDock } from "./AskDock";
export { useDockPrefetch } from "./dockPrefetch";
export type { KioskIdleOptions, KioskIdleState } from "./kioskIdle";
export { useKioskIdle } from "./kioskIdle";
export { workspaceLogoUrl } from "./logoUrl";
export { OXY_MARK_PATH, OxygenFactoryMark, OxyMark } from "./marks";
export { NavSheet } from "./NavSheet";
export type { OxyShellProps } from "./OxyShell";
export { OxyShell } from "./OxyShell";
export type { ReasoningTraceProps } from "./ReasoningTrace";
export { ReasoningTrace } from "./ReasoningTrace";
export type { RailItem } from "./ShellRail";
export { ShellRail } from "./ShellRail";
export type {
  ShellContextApp,
  ShellContextData,
  ShellContextUser,
  UseShellContextResult,
  ViewerIdentity
} from "./shellContext";
export { signOut, signOutUrl, useIdentity, useShellContext } from "./shellContext";
export { ShellTooltip } from "./Tooltip";
export { Breadcrumb, SystemIndicator, TopBar, WorkspaceClock } from "./TopBar";
export type {
  ThreadSummary,
  ThreadTranscript,
  TranscriptTurn,
  UseThreadHistoryResult
} from "./threadHistory";
export { fetchThreadTranscript, useThreadHistory } from "./threadHistory";
export type { TraceItem, TraceStep, TraceStepStatus } from "./trace";
export { buildTraceSteps } from "./trace";
export { UpdatePill } from "./UpdatePill";
export { WorkspaceTile } from "./WorkspaceTile";
