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
import { useOrgs } from "@/hooks/api/organizations";
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
export function factoryPath(target: FactoryTarget): string {
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
 * the org you are already acting as, picking a workspace just opens it. While acting
 * as one org, the staff console (and so the list of every org) is closed by the
 * server, so the picker offers that org's workspaces and says how to reach others.
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
  /** Skip the picker: the staff console's row action already knows the workspace. */
  initialTarget?: FactoryTarget | null;
  /** Open on one org's workspaces (the rail inside that org), with a way to search all. */
  initialScope?: PickerScope | null;
}) {
  const navigate = useNavigate();
  const { session, isActing } = useActingSession();
  const { data: myOrgs } = useOrgs();
  const [target, setTarget] = useState<FactoryTarget | null>(initialTarget);
  const [scope, setScope] = useState<PickerScope | null>(initialScope);
  // A scoped org with one workspace is picked once; Back then shows the list.
  const [autoPick, setAutoPick] = useState(true);

  // Reopening starts over (or at the row's workspace, or the rail's org).
  // Only on an open that follows a close: the first state already came from the
  // props, and resetting on mount would undo a pick the picker made on mount.
  const wasOpen = useRef(open);
  useEffect(() => {
    if (open && !wasOpen.current) {
      setTarget(initialTarget);
      setScope(initialScope);
      setAutoPick(true);
    }
    wasOpen.current = open;
  }, [open, initialTarget, initialScope]);

  const choose = (picked: FactoryTarget) => {
    const member = myOrgs?.some((o) => o.id === picked.orgId && !isActing) ?? false;
    const actingHere = isActing && session?.org_id === picked.orgId;
    if (member || actingHere) {
      onOpenChange(false);
      navigate(factoryPath(picked));
      return;
    }
    setTarget(picked);
  };

  // A real member never needs a session, nor does the org already being acted as.
  useEffect(() => {
    if (!open || !initialTarget) return;
    const member = myOrgs?.some((o) => o.id === initialTarget.orgId) ?? false;
    if (member) {
      onOpenChange(false);
      navigate(factoryPath(initialTarget));
    }
  }, [open, initialTarget, myOrgs, navigate, onOpenChange]);

  const startSession = (picked: FactoryTarget) => {
    rememberAssumeDestination({
      orgId: picked.orgId,
      landing: factoryPath(picked),
      returnTo: `${window.location.pathname}${window.location.search}`
    });
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
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
            onBack={
              initialTarget
                ? undefined
                : () => {
                    setAutoPick(false);
                    setTarget(null);
                  }
            }
            onBeforeStart={startSession}
          />
        ) : (
          <WorkspacePicker
            actingSession={isActing ? session : undefined}
            onPick={choose}
            scope={scope}
            autoPickSingle={autoPick}
            onClearScope={() => setScope(null)}
          />
        )}
      </DialogContent>
    </Dialog>
  );
}
