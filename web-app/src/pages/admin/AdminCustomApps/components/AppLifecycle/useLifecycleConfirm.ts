import { useState } from "react";
import type { MoveOverride } from "@/types/appPromote";
import type { LifecycleTarget } from "./LifecycleCard";
import type { ConfirmCopy } from "./lifecycleCopy";
import { overrideRefusal } from "./planChecks";

/** A verb as the confirm runs it; only a promote reads the `override`. */
export type ConfirmedRun = (override?: MoveOverride) => Promise<void>;

/**
 * The lifecycle card's state: what is about to happen (`pending`; `null` is
 * "nothing"), and why the server refused a promote past the checks (`refused`).
 *
 * `ask` opens the confirm on a verb, `close` drops it, and `confirm` runs the verb,
 * closing on success. `onTargetChange` is told which build the open confirm would put
 * live, and `null` when it closes.
 */
export function useLifecycleConfirm(onTargetChange?: (target: LifecycleTarget | null) => void) {
  // One piece of state: what is about to happen. `null` is "nothing".
  const [pending, setPending] = useState<{ copy: ConfirmCopy; run: ConfirmedRun } | null>(null);
  // Why the server refused a promote past the checks, said under its reason.
  const [refused, setRefused] = useState<string | null>(null);

  const ask = (copy: ConfirmCopy, run: ConfirmedRun, target?: LifecycleTarget) => {
    setPending({ copy, run });
    setRefused(null);
    onTargetChange?.(target ?? null);
  };
  const close = () => {
    setPending(null);
    setRefused(null);
    onTargetChange?.(null);
  };
  const confirm = async (override?: MoveOverride) => {
    if (!pending) return;
    setRefused(null);
    try {
      await (override ? pending.run(override) : pending.run());
      close();
    } catch (err) {
      // The verb toasted its own failure; the dialog stays so the operator can retry.
      // A refused override is the exception: it is said under its reason.
      setRefused(override ? overrideRefusal(err) : null);
    }
  };

  return { pending, refused, ask, close, confirm };
}

export type LifecycleConfirming = ReturnType<typeof useLifecycleConfirm>;
