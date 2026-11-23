import { create } from "zustand";
import useAskDock from "./useAskDock";

interface NotificationsDockState {
  /** Whether the panel takes width beside the page. It stays mounted while shut. */
  isOpen: boolean;
  open: () => void;
  close: () => void;
  toggle: () => void;
}

/**
 * The notifications panel, docked on the right like Ask and
 * opened by the top bar's bell. One panel at a time on that edge: opening this
 * one closes Ask, and the panel closes itself when Ask opens
 * (`NotificationsDock`).
 */
const useNotificationsDock = create<NotificationsDockState>()((set, get) => ({
  isOpen: false,
  open: () => {
    useAskDock.getState().close();
    set({ isOpen: true });
  },
  close: () => set({ isOpen: false }),
  toggle: () => (get().isOpen ? get().close() : get().open())
}));

export default useNotificationsDock;
