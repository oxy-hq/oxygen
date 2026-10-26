import { useEffect, useState } from "react";
import AddNamespaceMenu from "./AddNamespaceMenu";
import AddViaInstallationDialog from "./AddViaInstallationDialog";
import AddViaPATDialog from "./AddViaPATDialog";

type Step = "menu" | "pat" | "installation";

interface Props {
  orgId: string;
  /** Whether the caller is an owner or admin of `orgId`, who alone may attach an installation. */
  canUseApp: boolean;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onConnected: (namespaceId: string) => void;
}

export default function AddGitNamespaceFlow({
  orgId,
  canUseApp,
  open,
  onOpenChange,
  onConnected
}: Props) {
  const [step, setStep] = useState<Step>("menu");

  useEffect(() => {
    if (open) setStep("menu");
  }, [open]);

  const close = () => onOpenChange(false);

  return (
    <>
      <AddNamespaceMenu
        open={open && step === "menu"}
        canUseApp={canUseApp}
        onClose={close}
        onSelectApp={() => setStep("installation")}
        onSelectPAT={() => setStep("pat")}
      />
      <AddViaPATDialog
        orgId={orgId}
        open={open && step === "pat"}
        onClose={close}
        onConnected={onConnected}
      />
      <AddViaInstallationDialog
        orgId={orgId}
        open={open && canUseApp && step === "installation"}
        onClose={close}
        onConnected={onConnected}
      />
    </>
  );
}
