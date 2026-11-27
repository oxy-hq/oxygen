import { useState } from "react";
import { useMovePlan } from "@/hooks/api/customApps/useProductionMove";
import type { MoveOverride, MoveTarget } from "@/types/appPromote";
import { useAdminLifecycleActions } from "./actions";
import { LifecycleConfirm } from "./LifecycleConfirm";
import type { ConfirmCopy } from "./lifecycleCopy";
import { movePreflight } from "./movePreflight";
import { overrideRefusal } from "./planChecks";

/**
 * A production move confirmed on its dry-run plan, for a surface other than the
 * lifecycle card — the release timeline's Roll back. It is the card's confirm, not a
 * copy of it: the same plan read (`useMovePlan`), the same hold rule
 * (`movePreflight`), and the same act, plan-match guard and toasts
 * (`useAdminLifecycleActions`).
 *
 * `target` doubles as the open flag: `null` closes it. It stays open on a refused
 * act — the verb toasted why, and the plan is re-read — so the operator sees what
 * the server now says before trying again.
 */
export const PlannedMoveConfirm = ({
  appId,
  target,
  copy,
  onClose,
  testId
}: {
  appId: string;
  target: MoveTarget | null;
  copy: ConfirmCopy | null;
  onClose: () => void;
  testId?: string;
}) => {
  const plan = useMovePlan(appId, target);
  const actions = useAdminLifecycleActions(
    appId,
    target && plan.data ? { target, plan: plan.data } : undefined
  );
  const preflight = target ? movePreflight(plan) : undefined;
  const [refused, setRefused] = useState<string | null>(null);
  const close = () => {
    setRefused(null);
    onClose();
  };

  const confirm = async (override?: MoveOverride) => {
    if (!target) return;
    setRefused(null);
    try {
      if (target.act === "rollback") await actions.rollbackTo?.({ id: target.buildId });
      else await actions.promoteDraft?.(override);
      close();
    } catch (err) {
      // The verb toasted its own failure; the dialog stays so the operator can retry.
      // A refused override is the exception: it is said under its reason.
      setRefused(override ? overrideRefusal(err) : null);
    }
  };

  return (
    <LifecycleConfirm
      copy={target ? copy : null}
      pending={actions.pending}
      notice={preflight?.notice}
      hold={preflight?.hold ?? true}
      onConfirm={() => confirm()}
      onCancel={close}
      testId={testId}
      override={preflight?.override}
      onOverride={(reason) => confirm({ reason })}
      overrideError={refused}
    />
  );
};
