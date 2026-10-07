import { useEffect, useState } from "react";

/** How long the request must be in front of the person before Approve can be pressed. */
const ARM_DELAY_MS = 1000;

const attended = (): boolean => document.visibilityState === "visible" && document.hasFocus();

/**
 * Whether Approve may be pressed yet: there is a request to approve, and the page has been both
 * visible and focused for a full second since.
 *
 * oxyc opens this page at a moment it chooses, and the browser comes to the front. Without the
 * wait, a click or a key already on its way to another window could land on Approve. Hiding the
 * tab or leaving the window disarms it, and coming back counts the second again.
 *
 * @param ready There is something to approve. The second is counted from then, so Approve never
 *   turns on in the same instant the request appears.
 * @param approving What Approve would grant, where the person can change it on the page. A
 *   change disarms at once and counts a fresh second: a click already on its way to Approve
 *   must not approve something other than what was on the button when it set out. It is
 *   compared in render, so the button is off in the very frame that shows the new request.
 */
const useApprovalArmed = (ready: boolean, approving?: unknown): boolean => {
  // What the second was counted for, or `null` while it has not been counted.
  const [armedFor, setArmedFor] = useState<{ approving: unknown } | null>(null);

  useEffect(() => {
    if (!ready) return;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const count = () => {
      if (attended()) timer = setTimeout(() => setArmedFor({ approving }), ARM_DELAY_MS);
    };
    const recount = () => {
      clearTimeout(timer);
      setArmedFor(null);
      count();
    };
    count();
    document.addEventListener("visibilitychange", recount);
    window.addEventListener("focus", recount);
    window.addEventListener("blur", recount);
    return () => {
      clearTimeout(timer);
      setArmedFor(null);
      document.removeEventListener("visibilitychange", recount);
      window.removeEventListener("focus", recount);
      window.removeEventListener("blur", recount);
    };
  }, [ready, approving]);

  return ready && armedFor !== null && Object.is(armedFor.approving, approving);
};

export default useApprovalArmed;
