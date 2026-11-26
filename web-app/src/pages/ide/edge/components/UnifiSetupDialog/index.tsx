import { AlertCircle, Loader2 } from "lucide-react";
import type React from "react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { DESIGN_REFERENCE } from "@/components/ui/reference/tokens";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/shadcn/alert";
import { Button } from "@/components/ui/shadcn/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import useUnifiPreview from "@/hooks/api/cameras/useUnifiPreview";
import type { UnifiImportResult, UnifiPreviewResult } from "@/services/api";
import useCurrentWorkspace from "@/stores/useCurrentWorkspace";
import AirhouseGate from "./components/AirhouseGate";
import ApiKeyStep from "./components/ApiKeyStep";
import DoneStep from "./components/DoneStep";
import PreviewStep from "./components/PreviewStep";

/**
 * Multi-step UniFi onboarding state. Each variant carries the data the
 * next step needs — no separate refs/state for "the api key" or "the
 * preview result", just the union.
 *
 * `gate` is the entrypoint for fresh onboarding. The Airhouse-before-UniFi
 * check is structural (see project_camera_fleet_schema_gating in memory):
 * camera events have nowhere to land without a provisioned Airhouse tenant.
 *
 * `scan-loading` is the entrypoint when the dialog opens in `scan-stored`
 * mode — we kick off a preview call against the stored credential
 * immediately so the operator goes straight from "Scan sites" → preview
 * table without re-pasting their key.
 */
type Step =
  | { kind: "gate" }
  | { kind: "api-key" }
  | { kind: "scan-loading" }
  | { kind: "scan-error"; message: string }
  | { kind: "preview"; apiKey: string | undefined; data: UnifiPreviewResult }
  | { kind: "done"; result: UnifiImportResult };

type Props = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Fires when the wizard finishes; parent can switch tabs etc. */
  onImported?: (result: UnifiImportResult) => void;
  /**
   * `fresh` (default) — full onboarding wizard, starts at the Airhouse gate.
   * `scan-stored` — re-scan flow that assumes a stored UniFi credential and
   * skips straight to the preview. Used by the "Scan sites" action on the
   * configured-credential banner.
   */
  mode?: "fresh" | "scan-stored";
};

const UnifiSetupDialog: React.FC<Props> = ({ open, onOpenChange, onImported, mode = "fresh" }) => {
  const { workspace } = useCurrentWorkspace();
  const preview = useUnifiPreview(workspace?.id);
  const [step, setStep] = useState<Step>(() =>
    mode === "scan-stored" ? { kind: "scan-loading" } : { kind: "gate" }
  );

  // Reset internal state every time the dialog opens so re-entering
  // after a previous run doesn't show the old success page. Honours
  // the current `mode` so a banner click lands on scan-loading even
  // if the prior open used fresh mode.
  const handleOpenChange = (next: boolean) => {
    if (next && !open) {
      setStep(mode === "scan-stored" ? { kind: "scan-loading" } : { kind: "gate" });
    }
    onOpenChange(next);
  };

  // Kick off the stored-credential preview as soon as we land on
  // scan-loading. Guard against double-firing across StrictMode
  // mounts: the mutation hook's own pending flag would handle a
  // racing call but the effect cleanup keeps it tidy.
  useEffect(() => {
    if (step.kind !== "scan-loading") return;
    let cancelled = false;
    preview
      .mutateAsync({})
      .then((data) => {
        if (cancelled) return;
        setStep({ kind: "preview", apiKey: undefined, data });
      })
      .catch((err) => {
        if (cancelled) return;
        setStep({
          kind: "scan-error",
          message: err instanceof Error ? err.message : "Couldn't reach UniFi"
        });
      });
    return () => {
      cancelled = true;
    };
    // We intentionally only depend on the step kind. The preview
    // mutation reference can churn on re-renders without affecting
    // the request we want to fire.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [step.kind, preview.mutateAsync]);

  const handleImported = (result: UnifiImportResult) => {
    toast.success(
      `Imported ${result.sites_upserted} site${result.sites_upserted === 1 ? "" : "s"} and ${result.cameras_upserted} camera${result.cameras_upserted === 1 ? "" : "s"}.`
    );
    setStep({ kind: "done", result });
    onImported?.(result);
  };

  const handleClose = () => {
    onOpenChange(false);
  };

  const isScanMode = mode === "scan-stored";

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className={`${DESIGN_REFERENCE} max-w-2xl overflow-x-hidden`}>
        <DialogHeader>
          <DialogTitle>{isScanMode ? "Scan UniFi sites" : "Connect UniFi"}</DialogTitle>
          <DialogDescription>
            {isScanMode
              ? "Re-checking your UniFi account for new sites and cameras. Imports use the stored API key — no re-paste needed."
              : "Import your UniFi sites and cameras into this workspace."}
          </DialogDescription>
        </DialogHeader>

        {step.kind === "gate" && <AirhouseGate onReady={() => setStep({ kind: "api-key" })} />}

        {step.kind === "api-key" && (
          <ApiKeyStep onSuccess={(apiKey, data) => setStep({ kind: "preview", apiKey, data })} />
        )}

        {step.kind === "scan-loading" && (
          <div className='flex items-center justify-center gap-2 py-10 text-muted-foreground text-sm'>
            <Loader2 className='h-4 w-4 animate-spin' />
            Scanning UniFi account…
          </div>
        )}

        {step.kind === "scan-error" && (
          <div className='flex flex-col gap-4'>
            <Alert variant='destructive'>
              <AlertCircle />
              <AlertTitle>Couldn't reach UniFi</AlertTitle>
              <AlertDescription>{step.message}</AlertDescription>
            </Alert>
            <div className='flex justify-between'>
              <Button variant='ghost' onClick={handleClose}>
                Close
              </Button>
              <Button onClick={() => setStep({ kind: "scan-loading" })}>Retry</Button>
            </div>
          </div>
        )}

        {step.kind === "preview" && (
          <PreviewStep
            apiKey={step.apiKey}
            data={step.data}
            onBack={
              isScanMode
                ? () => setStep({ kind: "scan-loading" })
                : () => setStep({ kind: "api-key" })
            }
            onImported={handleImported}
          />
        )}

        {step.kind === "done" && <DoneStep result={step.result} onClose={handleClose} />}
      </DialogContent>
    </Dialog>
  );
};

export default UnifiSetupDialog;
