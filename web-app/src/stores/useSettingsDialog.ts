import { create } from "zustand";

export type SettingsSection =
  | "organization.general"
  | "organization.members"
  | "organization.teams"
  | "organization.app_access"
  | "organization.crew"
  | "organization.locations"
  | "organization.positions"
  | "organization.api_tokens"
  | "organization.secrets"
  | "organization.billing"
  | "organization.integration"
  | "workspace.members"
  | "workspace.databases"
  | "workspace.previews"
  | "workspace.airhouse"
  | "workspace.oltp"
  | "workspace.connections"
  | "workspace.apps"
  | "workspace.activity_logs"
  | "workspace.oxy_access"
  | "account.notifications"
  | "preferences.appearance";

interface SettingsDialogState {
  isOpen: boolean;
  section: SettingsSection;
  /**
   * The tab to show inside `section`, for a section that has tabs (`nav.ts` lists them). `null`
   * is the section's first tab the viewer may see. Cleared whenever the section changes without
   * naming one, so a tab never outlives the section it belongs to.
   */
  tab: string | null;
  open: (section?: SettingsSection, tab?: string | null) => void;
  /** Switch tabs inside the open section, without touching the section. */
  setTab: (tab: string | null) => void;
  close: () => void;
}

const useSettingsDialog = create<SettingsDialogState>()((set) => ({
  isOpen: false,
  section: "organization.general",
  tab: null,
  open: (section, tab) =>
    set((state) => {
      const next = section ?? state.section;
      // Re-opening the same section without a tab keeps the tab the viewer was on.
      const keepTab = tab === undefined && next === state.section;
      return { isOpen: true, section: next, tab: keepTab ? state.tab : (tab ?? null) };
    }),
  setTab: (tab) => set({ tab }),
  close: () => set({ isOpen: false })
}));

export default useSettingsDialog;
