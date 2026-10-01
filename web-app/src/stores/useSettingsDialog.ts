import { create } from "zustand";

export type SettingsSection =
  | "organization.general"
  | "organization.members"
  | "organization.teams"
  | "organization.app_access"
  | "organization.crew"
  | "organization.locations"
  | "organization.positions"
  | "organization.billing"
  | "organization.integration"
  | "workspace.members"
  | "workspace.databases"
  | "workspace.repositories"
  | "workspace.previews"
  | "workspace.airhouse"
  | "workspace.oltp"
  | "workspace.api_keys"
  | "workspace.secrets"
  | "workspace.connections"
  | "workspace.apps"
  | "workspace.activity_logs"
  | "workspace.oxy_access"
  | "preferences.appearance";

interface SettingsDialogState {
  isOpen: boolean;
  section: SettingsSection;
  open: (section?: SettingsSection) => void;
  close: () => void;
}

const useSettingsDialog = create<SettingsDialogState>()((set) => ({
  isOpen: false,
  section: "organization.general",
  open: (section) => set((state) => ({ isOpen: true, section: section ?? state.section })),
  close: () => set({ isOpen: false })
}));

export default useSettingsDialog;
