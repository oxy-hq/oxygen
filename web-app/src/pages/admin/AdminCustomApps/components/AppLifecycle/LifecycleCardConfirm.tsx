import type { LifecyclePreflight } from "./LifecycleCard";
import { LifecycleConfirm } from "./LifecycleConfirm";
import type { LifecycleConfirming } from "./useLifecycleConfirm";

/**
 * The card's confirm: the act `useLifecycleConfirm` holds, in the one dialog every
 * lifecycle action goes through. What the surface checked about the target
 * (`preflight`) is said, held on and offered only while a confirm is open.
 */
export const LifecycleCardConfirm = ({
  confirming,
  busy,
  preflight
}: {
  confirming: LifecycleConfirming;
  /** Any verb in flight. */
  busy: boolean;
  preflight?: LifecyclePreflight;
}) => {
  const { pending, refused, close, confirm } = confirming;
  return (
    <LifecycleConfirm
      copy={pending?.copy ?? null}
      pending={busy}
      onConfirm={() => confirm()}
      onCancel={close}
      notice={pending ? preflight?.notice : undefined}
      hold={!!pending && !!preflight?.hold}
      override={pending ? preflight?.override : undefined}
      onOverride={(reason) => confirm({ reason })}
      overrideError={refused}
    />
  );
};
