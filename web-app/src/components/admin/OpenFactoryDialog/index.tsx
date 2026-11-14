import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle
} from "@/components/ui/shadcn/dialog";
import { useActingSession } from "@/hooks/api/adminAssume/useActingSession";
import { useOpensFactoryDirectly } from "@/hooks/useStaffFactory";
import { rememberAssumeDestination } from "@/libs/utils/assumeDestination";
import ROUTES from "@/libs/utils/routes";
import { StartSessionStep } from "./components/StartSessionStep";
import {
  type FactoryTarget,
  type PickerScope,
  WorkspacePicker
} from "./components/WorkspacePicker";

export type { FactoryTarget, PickerScope } from "./components/WorkspacePicker";

/** The Factory of `target`, which an assumed session lands on. */
function factoryPath(target: FactoryTarget): string {
  return ROUTES.ORG(target.orgSlug).WORKSPACE(target.workspaceId).IDE.ROOT;
}

/**
 * "Open Factory…": an Oxy engineer picks any org's workspace by name and lands in
 * its Factory in one step.
 *
 * Staff hold no membership in a customer's org, so getting in takes an audited
 * assume-role session. Before this, opening a customer's Factory meant the staff
 * console's org list, assuming the org, landing on its home, and then finding the
 * Factory; following a link instead hit the "needs an assumed role" toast. Here the
 * session starts inline: pick the workspace, give the reason, and the session's
 * landing is that workspace's Factory (`rememberAssumeDestination`, consumed by
 * `useStartAssume`). Stopping the session brings you back to the page you opened
 * this from.
 *
 * When no session is needed it is skipped: in an org you are a real member of, or
 * the org you are already acting as, picking a workspace just opens it, and a
 * `initialTarget` that needs none never draws the dialog at all (it would only flash
 * open and shut). While acting as one org, the staff console (and so the list of
 * every org) is closed by the server, so the picker offers that org's workspaces and
 * says how to reach others.
 *
 * Staff only: workspace names are fine here; customers never see this.
 */
export function OpenFactoryDialog({
  open,
  onOpenChange,
  initialTarget = null,
  initialScope = null
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Start at this workspace's reason: the row's workspace, or the one the rail is in. */
  initialTarget?: FactoryTarget | null;
  /** The org `initialTarget` is in (the rail inside that org): Back lists its
   *  workspaces, with a way to search all. Without a target, the picker opens on it. */
  initialScope?: PickerScope | null;
}) {
  const navigate = useNavigate();
  const { session, isActing } = useActingSession();
  const opensDirectly = useOpensFactoryDirectly();
  const [target, setTarget] = useState<FactoryTarget | null>(initialTarget);
  const [scope, setScope] = useState<PickerScope | null>(initialScope);

  // Reopening starts over (at the row's workspace, or the rail's).
  const wasOpen = useRef(open);
  useEffect(() => {
    if (open && !wasOpen.current) {
      setTarget(initialTarget);
      setScope(initialScope);
    }
    wasOpen.current = open;
  }, [open, initialTarget, initialScope]);

  const choose = (picked: FactoryTarget) => {
    if (opensDirectly(picked.orgId)) {
      onOpenChange(false);
      navigate(factoryPath(picked));
      return;
    }
    setTarget(picked);
  };

  // A real member never needs a session, nor does the org already being acted as:
  // go straight in, without drawing the dialog first.
  const entersDirectly = !!initialTarget && opensDirectly(initialTarget.orgId);
  useEffect(() => {
    if (!open || !initialTarget || !entersDirectly) return;
    onOpenChange(false);
    navigate(factoryPath(initialTarget));
  }, [open, initialTarget, entersDirectly, navigate, onOpenChange]);

  const startSession = (picked: FactoryTarget) => {
    rememberAssumeDestination({
      orgId: picked.orgId,
      landing: factoryPath(picked),
      returnTo: `${window.location.pathname}${window.location.search}`
    });
  };

  return (
    <Dialog open={open && !entersDirectly} onOpenChange={onOpenChange}>
      <DialogContent className='overflow-hidden p-0 sm:max-w-lg' data-testid='open-factory-dialog'>
        <DialogHeader className={target ? "px-6 pt-6" : "sr-only"}>
          <DialogTitle>
            {target ? `Open ${target.workspaceName} in the Factory` : "Open Factory"}
          </DialogTitle>
          <DialogDescription>
            {target
              ? target.orgName
              : "Search organizations and their workspaces, then open one in the Factory."}
          </DialogDescription>
        </DialogHeader>
        {target ? (
          <StartSessionStep
            target={target}
            actingAs={isActing && session?.org_id !== target.orgId ? session : undefined}
            // The staff console's row knows its workspace: nothing to go back to.
            onBack={initialTarget && !initialScope ? undefined : () => setTarget(null)}
            onBeforeStart={startSession}
          />
        ) : (
          <WorkspacePicker
            actingSession={isActing ? session : undefined}
            onPick={choose}
            scope={scope}
            onClearScope={() => setScope(null)}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}
