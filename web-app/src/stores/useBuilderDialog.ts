import { create } from "zustand";

interface BuilderDialogState {
  isOpen: boolean;
  setIsOpen: (isOpen: boolean) => void;
}

const useBuilderDialog = create<BuilderDialogState>()((set) => ({
  isOpen: false,
  setIsOpen: (isOpen: boolean) => set({ isOpen })
}));

export default useBuilderDialog;
