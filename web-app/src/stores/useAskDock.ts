import { create } from "zustand";

interface AskPrefill {
  message?: string;
  agentPath?: string;
  autoSubmit?: boolean;
}

/** The body the dock currently shows. */
type AskDockView = "composer" | "thread" | "history";

interface AskDockState {
  /** Whether the dock occupies width. `false` collapses it to zero width
   *  while the dock stays mounted, so composer text and the live thread
   *  survive a collapse/expand (the "don't lose anything" requirement). */
  isOpen: boolean;
  /** Which view the dock body renders. */
  view: AskDockView;
  /** Thread bound to the in-dock thread view; survives view switches and
   *  collapse — only `newChat()` (or a fresh `open(prefill)`) clears it. */
  threadId: string | null;
  /** Composer prefill (suggestion click, onboarding deep-link). */
  prefill: AskPrefill | null;

  /** Open the dock. With a prefill, jump to the composer seeded with it;
   *  bare, restore whatever view was last shown. */
  open: (prefill?: AskPrefill) => void;
  /** Collapse to zero width, preserving view / thread / composer text. */
  close: () => void;
  /** Cmd+K / the Ask Oxygen button — flip open/closed, preserving state. */
  toggle: () => void;
  /** A thread was created (or a recent one selected) → show it in-dock. */
  openThread: (threadId: string) => void;
  /** Switch to the recent-threads list. */
  showHistory: () => void;
  /** Start over: empty composer, clear the bound thread. */
  newChat: () => void;
}

/**
 * Single store for the docked Ask panel — the merge of the old `useAskPanel`
 * (composer open/prefill) and `useThreadDrawer` (bound thread). One store
 * because the dock is now one surface with an internal view machine
 * (composer → thread → history), not two overlapping overlays.
 */
const useAskDock = create<AskDockState>()((set) => ({
  isOpen: false,
  view: "composer",
  threadId: null,
  prefill: null,

  open: (prefill) => set(prefill ? { isOpen: true, view: "composer", prefill } : { isOpen: true }),
  close: () => set({ isOpen: false }),
  toggle: () => set((s) => ({ isOpen: !s.isOpen })),
  openThread: (threadId) => set({ isOpen: true, view: "thread", threadId, prefill: null }),
  showHistory: () => set({ view: "history" }),
  newChat: () => set({ view: "composer", threadId: null, prefill: null })
}));

export default useAskDock;
